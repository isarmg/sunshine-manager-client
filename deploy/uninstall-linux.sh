#!/usr/bin/env bash
set -euo pipefail
if [[ $(id -u) != 0 || $# != 0 ]]; then echo "Run as root without arguments." >&2; exit 1; fi
if [[ -L /etc/systemd/system/xscc.service || -L /opt/xscc ]]; then
  echo "Unexpected installation links; refusing removal." >&2; exit 1
fi
if [[ -e /usr/local/bin/xscc || -L /usr/local/bin/xscc ]]; then
  [[ -L /usr/local/bin/xscc && $(readlink /usr/local/bin/xscc) = /opt/xscc/xscc ]] || exit 8
fi
grep -Fqx 'ExecStart=/opt/xscc/xscc run --state /var/lib/xscc' /etc/systemd/system/xscc.service || exit 8
timeout 60s systemctl disable --now xscc.service
if systemctl is-active --quiet xscc.service; then exit 9; fi
if [[ -L /usr/local/bin/xscc ]]; then rm /usr/local/bin/xscc; fi
rm -- /etc/systemd/system/xscc.service /opt/xscc/xscc
rmdir -- /opt/xscc
systemctl daemon-reload
echo "Client executable and service removed. State and service account retained for recovery/deduplication."
echo "Revoke the device in Manager. Do not purge /var/lib/xscc while operations remain unresolved."
