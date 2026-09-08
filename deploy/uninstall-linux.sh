#!/usr/bin/env bash
set -euo pipefail
if [[ $(id -u) != 0 || $# != 0 ]]; then echo "Run as root without arguments." >&2; exit 1; fi
if [[ -L /etc/systemd/system/sunshine-client.service || -L /opt/sunshine-client ]]; then
  echo "Unexpected installation links; refusing removal." >&2; exit 1
fi
if [[ -e /usr/local/bin/sunshine-client || -L /usr/local/bin/sunshine-client ]]; then
  [[ -L /usr/local/bin/sunshine-client && $(readlink /usr/local/bin/sunshine-client) = /opt/sunshine-client/sunshine-client ]] || exit 8
fi
grep -Fqx 'ExecStart=/opt/sunshine-client/sunshine-client run --state /var/lib/sunshine-client' /etc/systemd/system/sunshine-client.service || exit 8
timeout 60s systemctl disable --now sunshine-client.service
if systemctl is-active --quiet sunshine-client.service; then exit 9; fi
if [[ -L /usr/local/bin/sunshine-client ]]; then rm /usr/local/bin/sunshine-client; fi
rm -- /etc/systemd/system/sunshine-client.service /opt/sunshine-client/sunshine-client
rmdir -- /opt/sunshine-client
systemctl daemon-reload
echo "Client executable and service removed. State and service account retained for recovery/deduplication."
echo "Revoke the device in Manager. Do not purge /var/lib/sunshine-client while operations remain unresolved."
