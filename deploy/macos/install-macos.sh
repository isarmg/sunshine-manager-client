#!/bin/sh
set -eu
[ "$(uname -s)" = Darwin ] && [ "$(id -u)" = 0 ] && [ "$#" = 1 ] || { echo 'Usage: sudo install-macos.sh ABSOLUTE_VERIFIED_CLIENT_BINARY' >&2; exit 2; }
case "$1" in /*) ;; *) exit 2;; esac
[ -f "$1" ] && [ ! -L "$1" ] || exit 2
case "$("$1" --version)" in sunshine-client\ *) ;; *) echo 'Expected a verified Sunshine Client binary.' >&2; exit 2;; esac
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
[ -f "$source_dir/org.sarmg.sunshine-client.plist" ] && [ ! -L "$source_dir/org.sarmg.sunshine-client.plist" ] || exit 8
/usr/bin/plutil -lint "$source_dir/org.sarmg.sunshine-client.plist" >/dev/null
state='/Library/Application Support/sunshine-client'
plist='/Library/LaunchDaemons/org.sarmg.sunshine-client.plist'
binary='/usr/local/libexec/sunshine-client'
for path in "$state" "$plist" "$binary" /usr/local/bin/sunshine-client /var/log/sunshine-client.log /etc/newsyslog.d/sunshine-client.conf; do
  [ ! -e "$path" ] && [ ! -L "$path" ] || { echo 'Existing installation/state requires reviewed migration.' >&2; exit 5; }
done
if dscl . -read /Users/_sunshineclient >/dev/null 2>&1 || dscl . -read /Groups/_sunshineclient >/dev/null 2>&1; then
  echo 'Existing service account requires review; no account is modified.' >&2; exit 5
fi
# Refuse symlinked writable roots; do not repair ownership on existing directories.
for path in /Library /Library/LaunchDaemons "/Library/Application Support" /usr/local /usr/local/bin /usr/local/libexec /etc/newsyslog.d; do
  [ ! -L "$path" ] || exit 8
  if [ -d "$path" ]; then
    [ "$(stat -f %u "$path")" = 0 ] || exit 8
    mode=$(stat -f %Lp "$path")
    [ $((0$mode & 0022)) -eq 0 ] || exit 8
  fi
done
# System user/group IDs are checked independently and never repurposed.
uid=300
while [ "$uid" -lt 500 ]; do
  if ! dscl . -search /Users UniqueID "$uid" | grep -q . && ! dscl . -search /Groups PrimaryGroupID "$uid" | grep -q .; then break; fi
  uid=$((uid + 1))
done
[ "$uid" -lt 500 ] || exit 8
# Every rollback target was absent above. Track only successful creations;
# never recursively remove an existing product tree or a shared directory.
made_group=0 made_user=0 made_state=0 made_binary=0 made_link=0 made_plist=0 made_log=0 made_rotation=0 committed=0
rollback() {
  result=$?
  trap - EXIT HUP INT TERM
  if [ "$committed" = 0 ]; then
    set +e
    [ "$made_rotation" = 0 ] || rm -f /etc/newsyslog.d/sunshine-client.conf
    [ "$made_log" = 0 ] || rm -f /var/log/sunshine-client.log
    [ "$made_plist" = 0 ] || rm -f "$plist"
    [ "$made_link" = 0 ] || rm -f /usr/local/bin/sunshine-client
    [ "$made_binary" = 0 ] || rm -f "$binary"
    [ "$made_state" = 0 ] || rmdir "$state"
    [ "$made_user" = 0 ] || dscl . -delete /Users/_sunshineclient
    [ "$made_group" = 0 ] || dscl . -delete /Groups/_sunshineclient
    echo 'Installation failed; newly created Client artifacts rolled back.' >&2
  fi
  exit "$result"
}
trap rollback EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
dscl . -create /Groups/_sunshineclient
made_group=1
dscl . -create /Groups/_sunshineclient PrimaryGroupID "$uid"
dscl . -create /Users/_sunshineclient
made_user=1
dscl . -create /Users/_sunshineclient UniqueID "$uid"
dscl . -create /Users/_sunshineclient PrimaryGroupID "$uid"
dscl . -create /Users/_sunshineclient UserShell /usr/bin/false
dscl . -create /Users/_sunshineclient NFSHomeDirectory /var/empty
dscl . -create /Users/_sunshineclient IsHidden 1
install -d -m 0755 /usr/local/bin /usr/local/libexec
mkdir -m 0700 "$state"
made_state=1
chown _sunshineclient:_sunshineclient "$state"
made_binary=1
install -m 0755 -o root -g wheel "$1" "$binary"
ln -s "$binary" /usr/local/bin/sunshine-client
made_link=1
made_plist=1
install -m 0644 -o root -g wheel "$source_dir/org.sarmg.sunshine-client.plist" "$plist"
made_log=1
install -m 0600 -o _sunshineclient -g _sunshineclient /dev/null /var/log/sunshine-client.log
install -d -m 0755 /etc/newsyslog.d
made_rotation=1
printf '%s\n' '/var/log/sunshine-client.log _sunshineclient:_sunshineclient 600 7 1024 * J' > /etc/newsyslog.d/sunshine-client.conf
launchctl disable system/org.sarmg.sunshine-client
committed=1
echo 'Installed without pairing or service startup. Run sunshine-client pair --interactive, then sunshine-client service enable --now.'
