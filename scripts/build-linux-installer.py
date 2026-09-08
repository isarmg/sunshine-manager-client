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
        shutil.copy2(ROOT / 'deploy/configure-linux.py', stage / 'usr/bin/sunshine-client-setup')
        (stage / 'usr/bin/sunshine-client-setup').chmod(0o755)
        (stage / 'usr/bin/sunshine-client').symlink_to('/opt/sunshine-client/sunshine-client')
        shutil.copy2(ROOT / 'deploy/sunshine-client.service', stage / 'usr/lib/systemd/system/sunshine-client.service')
        shutil.copy2(ROOT / 'LICENSE-APACHE', stage / 'usr/share/doc/sunshine-client/copyright')
        (stage / 'DEBIAN/control').write_text(f'''Package: sunshine-client
Version: {version}
Architecture: amd64
Maintainer: sarmg <maintainers@sarmg.org>
Depends: libc6 (>= 2.39), libgcc-s1, libssl3t64, ca-certificates, python3, systemd, passwd
Section: admin
Priority: optional
Description: Sunshine management client
 Run sudo sunshine-client pair --interactive after installation.
 No video forwarding, inbound listener or changes to Sunshine.
''')
        scripts = {
            'preinst': '''#!/bin/sh
set -eu
if [ "$1" = upgrade ]; then echo 'Cross-version upgrades are not supported.' >&2; exit 1; fi
if [ "$1" = install ] && { [ -e /opt/sunshine-client ] || [ -e /var/lib/sunshine-client ] || [ -e /etc/systemd/system/sunshine-client.service ]; }; then
 echo 'Existing installation/state is preserved; refusing overwrite.' >&2; exit 1
fi
''',
            'postinst': '''#!/bin/sh
set -eu
if [ "$1" = configure ]; then
 if ! getent passwd sunshine-client >/dev/null; then useradd --system --no-create-home --home-dir /var/lib/sunshine-client --shell /usr/sbin/nologin sunshine-client; fi
 systemctl daemon-reload
 install -d -m 0700 -o sunshine-client -g sunshine-client /var/lib/sunshine-client
 echo 'Pair: sudo sunshine-client pair --interactive; then sudo sunshine-client service enable --now'
fi
''',
            'prerm': '''#!/bin/sh
set -eu
if [ "$1" = remove ]; then systemctl disable --now sunshine-client.service; fi
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
