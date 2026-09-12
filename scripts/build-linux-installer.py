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
        for directory in ['DEBIAN', 'opt/sunshine-client', 'usr/bin', 'usr/lib/systemd/system', 'usr/share/doc/sunshine-client']:
            (stage / directory).mkdir(parents=True)
        shutil.copy2(args.binary, stage / 'opt/sunshine-client/sunshine-client')
        (stage / 'usr/bin/sunshine-client').symlink_to('/opt/sunshine-client/sunshine-client')
        shutil.copy2(ROOT / 'deploy/sunshine-client.service', stage / 'usr/lib/systemd/system/sunshine-client.service')
        shutil.copy2(ROOT / 'LICENSE-APACHE', stage / 'usr/share/doc/sunshine-client/copyright')
        (stage / 'DEBIAN/control').write_text(f'''Package: sunshine-client
Version: {version}
Architecture: amd64
Maintainer: sarmg <maintainers@sarmg.org>
Depends: libc6 (>= 2.39), libgcc-s1, libssl3t64, ca-certificates, systemd, passwd
Section: admin
Priority: optional
Description: Sunshine management client
 Run sudo sunshine-client setup after installation.
 No video forwarding, inbound listener or changes to Sunshine.
''')
        scripts = {
            'preinst': '''#!/bin/sh
set -eu
for path in /opt/sunshine-client /var/lib/sunshine-client /etc/systemd/system/sunshine-client.service /usr/lib/systemd/system/sunshine-client.service; do
 [ ! -L "$path" ] || { echo "Redirected package path: $path" >&2; exit 8; }
done
if [ -f /etc/systemd/system/sunshine-client.service ]; then
 grep -Fqx 'ExecStart=/opt/sunshine-client/sunshine-client run --state /var/lib/sunshine-client' /etc/systemd/system/sunshine-client.service || exit 8
fi
''',
            'postinst': '''#!/bin/sh
set -eu
if [ "$1" = configure ]; then
 if ! getent passwd sunshine-client >/dev/null; then useradd --system --user-group --no-create-home --home-dir /var/lib/sunshine-client --shell /usr/sbin/nologin sunshine-client; fi
 entry=$(getent passwd sunshine-client)
 [ "$(printf '%s' "$entry" | cut -d: -f3)" != 0 ] || exit 8
 [ "$(printf '%s' "$entry" | cut -d: -f6)" = /var/lib/sunshine-client ] || exit 8
 [ "$(printf '%s' "$entry" | cut -d: -f7)" = /usr/sbin/nologin ] || exit 8
 if [ ! -e /var/lib/sunshine-client ]; then install -d -m 0700 -o sunshine-client -g sunshine-client /var/lib/sunshine-client; fi
 [ ! -L /var/lib/sunshine-client ] && [ -d /var/lib/sunshine-client ] || exit 8
 [ "$(stat -c %u /var/lib/sunshine-client)" = "$(id -u sunshine-client)" ] || exit 8
 if [ -f /etc/systemd/system/sunshine-client.service ]; then
  grep -Fqx 'ExecStart=/opt/sunshine-client/sunshine-client run --state /var/lib/sunshine-client' /etc/systemd/system/sunshine-client.service || exit 8
  rm /etc/systemd/system/sunshine-client.service
 fi
 systemctl daemon-reload
 if [ -f /run/sunshine-client-package-was-active ] && [ ! -L /run/sunshine-client-package-was-active ]; then
  systemctl start sunshine-client.service
  rm /run/sunshine-client-package-was-active
 fi
 if [ -t 0 ] && [ -t 1 ]; then
  if ! /usr/bin/sunshine-client setup; then echo 'Setup was not completed; installation and pairing progress were retained.' >&2; fi
 else
  echo 'No interactive terminal was available; resume with: sudo sunshine-client setup'
 fi
fi
''',
            'prerm': '''#!/bin/sh
set -eu
if [ "$1" = upgrade ]; then
 if systemctl is-active --quiet sunshine-client.service; then
  [ ! -e /run/sunshine-client-package-was-active ] && [ ! -L /run/sunshine-client-package-was-active ] || exit 8
  (umask 077; set -C; : > /run/sunshine-client-package-was-active)
 fi
 systemctl stop sunshine-client.service
elif [ "$1" = remove ]; then systemctl disable --now sunshine-client.service; fi
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
        subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(stage), str(args.output / f'sunshine-client_{args.version}_amd64.deb')], check=True)


if __name__ == '__main__':
    main()
