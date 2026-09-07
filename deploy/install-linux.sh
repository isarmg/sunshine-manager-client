#!/usr/bin/env bash
set -euo pipefail
# Explicit local installation only. Never invoked from a Manager task.
if [[ $(id -u) != 0 || $(uname -s) != Linux || $(uname -m) != x86_64 || $# != 2 ]]; then
  echo "Usage (root, Linux x86_64): install-linux.sh ABSOLUTE_CLIENT_BINARY ABSOLUTE_PROTECTED_BOOTSTRAP" >&2
  exit 1
fi
binary=$1
bootstrap=$2
if [[ $binary != /* || $bootstrap != /* || ! -f $binary || -L $binary || ! -f $bootstrap || -L $bootstrap ]]; then exit 1; fi
if [[ $("$binary" --version) != sunshine-client\ * ]]; then
  echo "Expected a verified Sunshine Client binary." >&2
  exit 1
fi
for target in /opt/sunshine-client /var/lib/sunshine-client /etc/systemd/system/sunshine-client.service; do
  if [[ -e $target || -L $target ]]; then
    echo "Refusing to overwrite an existing installation or state. Upgrade/restore belongs to sarmg-upgrade." >&2
    exit 1
  fi
done
if getent passwd sunshine-client >/dev/null; then
  echo "Service account already exists; review it before installing." >&2
  exit 1
fi
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
install -d -m 0755 /opt/sunshine-client
install -m 0755 -- "$binary" /opt/sunshine-client/sunshine-client
/opt/sunshine-client/sunshine-client init --state /var/lib/sunshine-client --bootstrap "$bootstrap"
useradd --system --user-group --home-dir /var/lib/sunshine-client --shell /usr/sbin/nologin sunshine-client
# This directory was created above and was explicitly required not to exist.
chown -R sunshine-client:sunshine-client /var/lib/sunshine-client
install -m 0644 "$script_dir/sunshine-client.service" /etc/systemd/system/sunshine-client.service
systemctl daemon-reload
systemctl enable --now sunshine-client.service
systemctl is-active --quiet sunshine-client.service
echo "Client installed. Verify registration in Manager, then securely remove the original bootstrap."
