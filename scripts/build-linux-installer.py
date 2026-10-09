#!/usr/bin/env python3
"""Build an Ubuntu 24.04 amd64 DEB from the already source-verified binary."""
import argparse
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:-rc\.\d+)?', args.version):
        parser.error('invalid version')
    version = args.version.replace('-rc.', '~rc')
    with tempfile.TemporaryDirectory(prefix='sunshine-deb-') as temporary:
        stage = Path(temporary)
        stage.chmod(0o755)
        for directory in ['DEBIAN', 'opt/xscc', 'usr/bin', 'usr/lib/systemd/system', 'usr/share/doc/xscc']:
            (stage / directory).mkdir(parents=True)
        shutil.copy2(args.binary, stage / 'opt/xscc/xscc')
        (stage / 'usr/bin/xscc').symlink_to('/opt/xscc/xscc')
        shutil.copy2(ROOT / 'deploy/xscc.service', stage / 'usr/lib/systemd/system/xscc.service')
        shutil.copy2(ROOT / 'LICENSE-APACHE', stage / 'usr/share/doc/xscc/copyright')
        (stage / 'DEBIAN/control').write_text(f'''Package: xscc
Version: {version}
Architecture: amd64
Maintainer: sarmg <maintainers@sarmg.org>
Depends: libc6 (>= 2.39), libgcc-s1, libssl3t64, ca-certificates, systemd, passwd
Section: admin
Priority: optional
Description: Sunshine management client
 Run sudo xscc setup after installation.
 No video forwarding, inbound listener or changes to Sunshine.
''')
        scripts = {
            'preinst': '''#!/bin/sh
set -eu
for path in /opt/xscc /var/lib/xscc /etc/systemd/system/xscc.service /usr/lib/systemd/system/xscc.service; do
 [ ! -L "$path" ] || { echo "Redirected package path: $path" >&2; exit 8; }
done
if [ -f /etc/systemd/system/xscc.service ]; then
 grep -Fqx 'ExecStart=/opt/xscc/xscc run --state /var/lib/xscc' /etc/systemd/system/xscc.service || exit 8
 # Retain the administrator's explicit startup choice when importing a script install.
 policy=/etc/systemd/system/.xscc-package-startup-policy
 if [ ! -e "$policy" ] && [ ! -L "$policy" ]; then
  if systemctl is-enabled --quiet xscc.service; then startup=enabled; else startup=disabled; fi
  (umask 077; set -C; printf '%s\\n' "$startup" > "$policy")
 fi
 [ -f "$policy" ] && [ ! -L "$policy" ] && [ "$(stat -c %u "$policy")" = 0 ] && [ "$(stat -c %a "$policy")" = 600 ] || exit 8
 case "$(cat "$policy")" in enabled|disabled) ;; *) exit 8;; esac
fi
''',
            'postinst': '''#!/bin/sh
set -eu
if [ "$1" = configure ]; then
 if ! getent passwd xscc >/dev/null; then useradd --system --user-group --no-create-home --home-dir /var/lib/xscc --shell /usr/sbin/nologin xscc; fi
 entry=$(getent passwd xscc)
 [ "$(printf '%s' "$entry" | cut -d: -f3)" != 0 ] || exit 8
 [ "$(printf '%s' "$entry" | cut -d: -f6)" = /var/lib/xscc ] || exit 8
 [ "$(printf '%s' "$entry" | cut -d: -f7)" = /usr/sbin/nologin ] || exit 8
 if [ ! -e /var/lib/xscc ]; then install -d -m 0700 -o xscc -g xscc /var/lib/xscc; fi
 [ ! -L /var/lib/xscc ] && [ -d /var/lib/xscc ] || exit 8
 [ "$(stat -c %u /var/lib/xscc)" = "$(id -u xscc)" ] || exit 8
 if [ -f /etc/systemd/system/xscc.service ]; then
  grep -Fqx 'ExecStart=/opt/xscc/xscc run --state /var/lib/xscc' /etc/systemd/system/xscc.service || exit 8
  rm /etc/systemd/system/xscc.service
 fi
 systemctl daemon-reload
 policy=/etc/systemd/system/.xscc-package-startup-policy
 if [ -e "$policy" ] || [ -L "$policy" ]; then
  [ -f "$policy" ] && [ ! -L "$policy" ] && [ "$(stat -c %u "$policy")" = 0 ] && [ "$(stat -c %a "$policy")" = 600 ] || exit 8
  case "$(cat "$policy")" in
   enabled) systemctl enable --force xscc.service;;
   disabled) systemctl disable xscc.service;;
   *) exit 8;;
  esac
 elif [ -z "${2:-}" ]; then
  # Fresh installs opt in to boot startup; pairing still happens only in setup.
  systemctl enable xscc.service
 fi
 if [ -f /run/xscc-package-was-active ] && [ ! -L /run/xscc-package-was-active ]; then
  systemctl start xscc.service
  rm /run/xscc-package-was-active
 fi
 [ ! -f "$policy" ] || rm "$policy"
 if [ -t 0 ] && [ -t 1 ]; then
  if ! /usr/bin/xscc setup; then echo 'Setup was not completed; installation and pairing progress were retained.' >&2; fi
 else
  echo 'No interactive terminal was available; resume with: sudo xscc setup'
 fi
fi
''',
            'prerm': '''#!/bin/sh
set -eu
if [ "$1" = upgrade ]; then
 if systemctl is-active --quiet xscc.service; then
  [ ! -e /run/xscc-package-was-active ] && [ ! -L /run/xscc-package-was-active ] || exit 8
  (umask 077; set -C; : > /run/xscc-package-was-active)
 fi
 systemctl stop xscc.service
elif [ "$1" = remove ]; then systemctl disable --now xscc.service; fi
''',
            'postrm': '''#!/bin/sh
set -eu
systemctl daemon-reload
echo 'Device identity and journal retained. Revoke the device in Manager before retirement.'
''',
        }
        for name, contents in scripts.items():
            path = stage / 'DEBIAN' / name
            path.write_text(contents)
            path.chmod(0o755)
        # Keep Debian's tilde ordering in control, but use a GitHub-safe asset name.
        subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(stage), str(args.output / f'xscc_{args.version}_amd64.deb')], check=True)


if __name__ == '__main__':
    main()
