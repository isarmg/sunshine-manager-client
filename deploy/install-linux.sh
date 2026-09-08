#!/usr/bin/env bash
set -euo pipefail
# Explicit local installation only. Never invoked from a Manager task.
if [[ $(id -u) != 0 || $(uname -s) != Linux || $(uname -m) != x86_64 || ( $# != 1 && $# != 2 ) ]]; then
  echo "Usage (root, Linux x86_64): install-linux.sh ABSOLUTE_CLIENT_BINARY [ABSOLUTE_PROTECTED_BOOTSTRAP]" >&2
  exit 1
fi
binary=$1
bootstrap=${2:-}
if [[ $binary != /* || ! -f $binary || -L $binary ]]; then exit 1; fi
if [[ -n $bootstrap && ( $bootstrap != /* || ! -f $bootstrap || -L $bootstrap ) ]]; then exit 1; fi
if [[ $("$binary" --version) != sunshine-client\ * ]]; then
  echo "Expected a verified Sunshine Client binary." >&2
  exit 1
fi
for target in /opt/sunshine-client /var/lib/sunshine-client /etc/systemd/system/sunshine-client.service /usr/local/bin/sunshine-client; do
  if [[ -e $target || -L $target ]]; then
    echo "Refusing to overwrite an existing installation or state. Upgrade/restore belongs to sarmg-upgrade." >&2
    exit 1
  fi
done
if getent passwd sunshine-client >/dev/null || getent group sunshine-client >/dev/null; then
  echo "Service account already exists; review it before installing." >&2
  exit 1
fi
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
[ -f "$script_dir/sunshine-client.service" ] && [ ! -L "$script_dir/sunshine-client.service" ]
for parent in /opt /var/lib /etc/systemd/system /usr/local/bin; do
  [[ -d $parent && ! -L $parent && $(stat -c %u "$parent") = 0 ]] || exit 8
  mode=$(stat -c %a "$parent")
  (( (8#$mode & 0022) == 0 )) || exit 8
done
made_binary=0 made_state=0 made_user=0 made_unit=0 made_link=0 committed=0
rollback() {
  result=$?
  trap - EXIT HUP INT TERM
  if [[ $committed = 0 ]]; then
    set +e
    [[ $made_link = 0 ]] || rm -f /usr/local/bin/sunshine-client
    if [[ $made_unit = 1 ]]; then
      rm -f /etc/systemd/system/sunshine-client.service
      systemctl daemon-reload
    fi
    # These private directories were created by this invocation, before importing
    # bootstrap. No pairing or execution has run and no preexisting state is removed.
    [[ $made_state = 0 ]] || rm -rf -- /var/lib/sunshine-client
    [[ $made_binary = 0 ]] || rm -rf -- /opt/sunshine-client
    if [[ $made_user = 1 ]]; then
      userdel sunshine-client
      if getent group sunshine-client >/dev/null; then groupdel sunshine-client; fi
    fi
    echo 'Installation failed; newly created Client artifacts rolled back.' >&2
  fi
  exit "$result"
}
trap rollback EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
mkdir -m 0755 /opt/sunshine-client
made_binary=1
mkdir -m 0700 /var/lib/sunshine-client
made_state=1
install -m 0755 -- "$binary" /opt/sunshine-client/sunshine-client
if [[ -n $bootstrap ]]; then
  /opt/sunshine-client/sunshine-client init --state /var/lib/sunshine-client --bootstrap "$bootstrap"
fi
useradd --system --user-group --home-dir /var/lib/sunshine-client --shell /usr/sbin/nologin sunshine-client
made_user=1
# This directory was created above and was explicitly required not to exist.
chown -R sunshine-client:sunshine-client /var/lib/sunshine-client
made_unit=1
install -m 0644 "$script_dir/sunshine-client.service" /etc/systemd/system/sunshine-client.service
systemctl daemon-reload
ln -s /opt/sunshine-client/sunshine-client /usr/local/bin/sunshine-client
made_link=1
committed=1
echo "Client installed. Run sunshine-client pair --interactive, then sunshine-client service enable --now."
