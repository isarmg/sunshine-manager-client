#!/usr/bin/env bash
set -euo pipefail
if [[ $(id -u) != 0 || $# != 0 ]]; then echo "Run as root without arguments." >&2; exit 1; fi
if [[ -L /etc/systemd/system/sunshine-client.service || -L /opt/sunshine-client ]]; then
  echo "Unexpected installation links; refusing removal." >&2; exit 1
fi
systemctl disable --now sunshine-client.service
rm -- /etc/systemd/system/sunshine-client.service /opt/sunshine-client/sunshine-client
rmdir -- /opt/sunshine-client
systemctl daemon-reload
echo "Client executable and service removed. State and service account retained for recovery/deduplication."
echo "Revoke the device in Manager. Do not purge /var/lib/sunshine-client while operations remain unresolved."
