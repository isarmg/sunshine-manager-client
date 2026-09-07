#!/usr/bin/python3
"""Interactive first-time pairing; passwords never enter shell arguments."""
import getpass
import json
import os
from pathlib import Path
import pwd
import subprocess
import tempfile


def main():
    if os.geteuid() != 0:
        raise SystemExit("Run sudo sunshine-client-setup")
    state = Path('/var/lib/sunshine-client')
    if state.exists():
        raise SystemExit("Existing state is preserved. Use Manager to inspect or revoke the device.")
    print("Sunshine Client pairing / Sunshine Client 配对")
    config = {}
    for key, label, default in [
        ('manager_endpoint', 'Manager WSS URL / 地址', ''),
        ('manager_id', 'Manager ID / 标识', ''),
        ('device_id', 'Device ID / 设备标识', ''),
        ('sunshine_endpoint', 'Sunshine HTTPS URL / 本机地址', 'https://127.0.0.1:47990/'),
        ('sunshine_username', 'Sunshine username / 用户名', ''),
    ]:
        config[key] = input(f'{label} [{default}]: ').strip() or default
    config['enrollment_token'] = getpass.getpass('Pairing code / 配对码: ')
    config['sunshine_password'] = getpass.getpass('Sunshine password / 密码: ')
    for key in ('manager_ca_pem', 'sunshine_ca_pem'):
        source = Path(input(f'{key} — verified PEM path / 已核验证书路径: '))
        with source.open('r') as stream:
            certificate = stream.read(65537)
        if len(certificate) > 65536:
            raise SystemExit('Certificate too large / 证书过大')
        config[key] = certificate
    config['restart_allowed'] = False
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix='sunshine-pairing-') as temporary:
        bootstrap = Path(temporary) / 'bootstrap.json'
        bootstrap.write_text(json.dumps(config))
        subprocess.run(['/opt/sunshine-client/sunshine-client', 'init', '--state', str(state), '--bootstrap', str(bootstrap)], check=True)
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
