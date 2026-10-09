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
if [[ $("$binary" --version) != xscc\ * ]]; then
  echo "Expected a verified xscc binary." >&2
  exit 1
fi
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
for target in /opt/xscc /var/lib/xscc /etc/systemd/system/xscc.service /usr/lib/systemd/system/xscc.service /usr/local/bin/xscc; do
  if [[ -e $target || -L $target ]]; then
    exec bash "$script_dir/repair-existing.sh" "$binary" "$script_dir"
  fi
done
if getent passwd xscc >/dev/null || getent group xscc >/dev/null; then
  echo 'Service account exists without protected state; inspection required.' >&2; exit 8
fi
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
[ -f "$script_dir/xscc.service" ] && [ ! -L "$script_dir/xscc.service" ]
for parent in /opt /var/lib /etc/systemd/system /usr/local/bin; do
  [[ -d $parent && ! -L $parent && $(stat -c %u "$parent") = 0 ]] || exit 8
  mode=$(stat -c %a "$parent")
  (( (8#$mode & 0022) == 0 )) || exit 8
done
made_binary=0 made_state=0 made_user=0 made_unit=0 made_link=0 made_enable=0 committed=0
rollback() {
  result=$?
  trap - EXIT HUP INT TERM
  if [[ $committed = 0 ]]; then
    set +e
    # enable may have created only some startup links before returning an error.
    [[ $made_enable = 0 ]] || systemctl disable --now xscc.service
    [[ $made_link = 0 ]] || rm -f /usr/local/bin/xscc
    if [[ $made_unit = 1 ]]; then
      rm -f /etc/systemd/system/xscc.service
      systemctl daemon-reload
    fi
    # These private directories were created by this invocation, before importing
    # bootstrap. No pairing or execution has run and no preexisting state is removed.
    [[ $made_state = 0 ]] || rm -rf -- /var/lib/xscc
    [[ $made_binary = 0 ]] || rm -rf -- /opt/xscc
    if [[ $made_user = 1 ]]; then
      userdel xscc
      if getent group xscc >/dev/null; then groupdel xscc; fi
    fi
    echo 'Installation failed; newly created Client artifacts rolled back.' >&2
  fi
  exit "$result"
}
trap rollback EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
mkdir -m 0755 /opt/xscc
made_binary=1
mkdir -m 0700 /var/lib/xscc
made_state=1
install -m 0755 -- "$binary" /opt/xscc/xscc
if [[ -n $bootstrap ]]; then
  /opt/xscc/xscc init --state /var/lib/xscc --bootstrap "$bootstrap"
fi
useradd --system --user-group --home-dir /var/lib/xscc --shell /usr/sbin/nologin xscc
made_user=1
# This directory was created above and was explicitly required not to exist.
chown -R xscc:xscc /var/lib/xscc
made_unit=1
install -m 0644 "$script_dir/xscc.service" /etc/systemd/system/xscc.service
systemctl daemon-reload
ln -s /opt/xscc/xscc /usr/local/bin/xscc
made_link=1
made_enable=1
systemctl enable xscc.service
committed=1
echo "Client installed with systemd startup enabled. Complete pairing with xscc setup --interactive; the unpaired service remains stopped."
