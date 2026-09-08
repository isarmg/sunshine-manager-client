#!/usr/bin/env bash
# Called only after the platform installer validates root and source binary.
set -euo pipefail
source_binary=$1
resource_dir=$2
os=$(uname -s)
if [[ $os = Linux ]]; then
  state=/var/lib/sunshine-client
  target=/opt/sunshine-client/sunshine-client
  unit=/etc/systemd/system/sunshine-client.service
  # Respect DEB ownership: repair the package unit when one is present.
  [[ ! -f /usr/lib/systemd/system/sunshine-client.service ]] || unit=/usr/lib/systemd/system/sunshine-client.service
  source_unit=$resource_dir/sunshine-client.service
  account=sunshine-client
  group=sunshine-client
  entry=$(getent passwd "$account") || { echo 'Existing state has no service account; retained for inspection.' >&2; exit 8; }
  IFS=: read -r _ _ service_uid service_gid _ service_home service_shell <<<"$entry"
  [[ $service_home = "$state" && $service_shell = /usr/sbin/nologin && $service_uid != 0 ]] || exit 8
  [[ $(getent group "$group" | cut -d: -f3) = "$service_gid" ]] || exit 8
  meta() { stat -c "$1" "$2"; }
  uid_format=%u; mode_format=%a
else
  [[ $os = Darwin && $(uname -m) = arm64 ]] || exit 8
  state='/Library/Application Support/sunshine-client'
  target=/usr/local/libexec/sunshine-client
  unit=/Library/LaunchDaemons/org.sarmg.sunshine-client.plist
  source_unit=$resource_dir/org.sarmg.sunshine-client.plist
  account=_sunshineclient; group=_sunshineclient
  service_uid=$(dscl . -read /Users/$account UniqueID | awk '{print $2}')
  service_gid=$(dscl . -read /Groups/$group PrimaryGroupID | awk '{print $2}')
  [[ $service_uid != 0 && $service_uid =~ ^[0-9]+$ && $service_gid =~ ^[0-9]+$ ]] || exit 8
  [[ $(dscl . -read /Users/$account PrimaryGroupID | awk '{print $2}') = "$service_gid" ]] || exit 8
  [[ $(dscl . -read /Users/$account UserShell | awk '{print $2}') = /usr/bin/false ]] || exit 8
  meta() { stat -f "$1" "$2"; }
  uid_format=%u; mode_format=%Lp
fi
# Validate every ancestor, then only product-owned leaves. Never recurse through
# the existing state tree, replace credentials, or follow a redirected target.
check_parent() {
  local p=$1 mode
  while [[ $p != / ]]; do
    if [[ ! -e $p && ! -L $p ]]; then p=$(dirname "$p"); continue; fi
    [[ -d $p && ! -L $p && $(meta "$uid_format" "$p") = 0 ]] || exit 8
    mode=$(meta "$mode_format" "$p"); (( (8#$mode & 0022) == 0 )) || exit 8
    p=$(dirname "$p")
  done
}
for path in "$target" "$unit" "$state" /usr/local/bin/sunshine-client; do check_parent "$(dirname "$path")"; done
[[ -d $state && ! -L $state && $(meta "$uid_format" "$state") = "$service_uid" ]] || exit 8
mode=$(meta "$mode_format" "$state"); (( (8#$mode & 0077) == 0 )) || exit 8
for path in "$target" "$unit"; do
  if [[ -e $path || -L $path ]]; then
    [[ -f $path && ! -L $path && $(meta "$uid_format" "$path") = 0 ]] || exit 8
  fi
done
link=/usr/local/bin/sunshine-client
if [[ -e $link || -L $link ]]; then [[ -L $link && $(readlink "$link") = "$target" ]] || exit 8; fi
if [[ -f $unit ]]; then
  if [[ $os = Linux ]]; then
    grep -Fqx 'ExecStart=/opt/sunshine-client/sunshine-client run --state /var/lib/sunshine-client' "$unit" || exit 8
  else
    [[ $(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' "$unit") = "$target" ]] || exit 8
  fi
fi
# Stage in an administrator-only directory. A failed replacement restores the
# exact previous program/service files; user data is never a rollback target.
backup=$(mktemp -d "$(dirname "$unit")/.sunshine-repair.XXXXXX")
chmod 700 "$backup"
had_target=0; had_unit=0; had_link=0; active=0; committed=0; stopped=0
[[ ! -f $target ]] || { cp -p "$target" "$backup/binary"; had_target=1; }
[[ ! -f $unit ]] || { cp -p "$unit" "$backup/unit"; had_unit=1; }
[[ ! -L $link ]] || had_link=1
if [[ $os = Linux ]]; then
  if systemctl is-active --quiet sunshine-client.service; then active=1; fi
else
  if launchctl print system/org.sarmg.sunshine-client >/dev/null 2>&1; then active=1; fi
fi
reload_service() {
  if [[ $os = Linux ]]; then
    systemctl daemon-reload
    [[ $active = 0 ]] || systemctl start sunshine-client.service
  elif [[ $active = 1 ]]; then
    launchctl bootstrap system "$unit"
  fi
}
rollback() {
  result=$?; trap - EXIT
  if [[ $committed = 0 ]]; then
    set +e
    if [[ $had_target = 1 ]]; then cp -p "$backup/binary" "$target"; else rm -f "$target"; fi
    if [[ $had_unit = 1 ]]; then cp -p "$backup/unit" "$unit"; else rm -f "$unit"; fi
    [[ $had_link = 1 ]] || rm -f "$link"
    [[ $stopped = 0 ]] || reload_service
    echo "Repair failed; previous files restored, protected backup retained: $backup" >&2
  else
    rm -rf "$backup"
  fi
  exit "$result"
}
trap rollback EXIT
if [[ $os = Linux ]]; then systemctl stop sunshine-client.service; elif [[ $active = 1 ]]; then launchctl bootout system/org.sarmg.sunshine-client; fi
stopped=1
install -d -m 0755 "$(dirname "$target")"
install -m 0755 "$source_binary" "$backup/new-binary"
# rename replaces read-only/damaged files without opening a running executable.
mv -f "$backup/new-binary" "$target"
install -m 0644 "$source_unit" "$backup/new-unit"
mv -f "$backup/new-unit" "$unit"
[[ $had_link = 1 ]] || ln -s "$target" "$link"
reload_service
committed=1
echo 'Client program and service repaired; identity, credentials, task journal and startup policy retained.'
