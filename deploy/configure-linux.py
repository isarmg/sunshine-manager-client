#!/usr/bin/python3
"""Interactive first-time pairing; passwords never enter shell arguments."""
import getpass
import json
import os
from pathlib import Path
import pwd
import subprocess
import tempfile
from urllib.parse import urlsplit, urlunsplit


def main():
    if os.geteuid() != 0:
        raise SystemExit("Run sudo sunshine-client-setup")
    state = Path('/var/lib/sunshine-client')
    print("Sunshine Client pairing / Sunshine Client 配对")
    config = {}
    for key, label, default in [
        ('manager_endpoint', 'Server address / 服务器地址', ''),
        ('sunshine_endpoint', 'Sunshine loopback IP / 本机回环地址', '127.0.0.1'),
        ('sunshine_username', 'Sunshine username / 用户名', ''),
    ]:
        config[key] = input(f'{label} [{default}]: ').strip() or default
    port = int(input('Sunshine port / 端口 [47990]: ').strip() or '47990')
    if not 1 <= port <= 65535:
        raise ValueError('invalid port')
    address = config['manager_endpoint']
    server = urlsplit(address if '://' in address else 'https://' + address)
    if server.scheme not in ('https', 'wss') or not server.hostname or server.username or server.password or server.query or server.fragment:
        raise ValueError('invalid server address')
    config['manager_endpoint'] = urlunsplit(('wss', server.netloc, '/sunshine-client/v1/connect', '', ''))
    host = config['sunshine_endpoint']
    config['sunshine_endpoint'] = f'https://{host if ":" not in host else "[" + host + "]"}:{port}/'
    config['enrollment_token'] = getpass.getpass('Pairing code / 配对码: ')
    config['sunshine_password'] = getpass.getpass('Sunshine password / 密码: ')
    config['restart_allowed'] = False
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix='sunshine-pairing-') as temporary:
        bootstrap = Path(temporary) / 'bootstrap.json'
        bootstrap.write_text(json.dumps(config))
        subprocess.run(['/opt/sunshine-client/sunshine-client', 'init', '--state', str(state), '--bootstrap', str(bootstrap)], check=True)
    subprocess.run(['/opt/sunshine-client/sunshine-client', 'pair', '--state', str(state)], check=True)
    account = pwd.getpwnam('sunshine-client')
    # Only newly initialized state is transferred; no existing identity is overwritten.
    for directory, dirs, files in os.walk(state, followlinks=False):
        os.chown(directory, account.pw_uid, account.pw_gid, follow_symlinks=False)
        for name in files:
            os.chown(Path(directory) / name, account.pw_uid, account.pw_gid, follow_symlinks=False)
    subprocess.run(['systemctl', 'enable', '--now', 'sunshine-client.service'], check=True)
    print('Service started. Verify connectivity and Sunshine status in Manager. / 服务已启动，请在 Manager 核验连接与 Sunshine 状态。')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError, KeyboardInterrupt):
        raise SystemExit('Setup incomplete. Check inputs and certificates. / 配置未完成，请检查输入与证书。') from None
