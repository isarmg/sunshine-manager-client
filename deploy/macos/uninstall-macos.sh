#!/bin/sh
set -eu
[ "$(uname -s)" = Darwin ] && [ "$(id -u)" = 0 ] || exit 3
plist='/Library/LaunchDaemons/org.sarmg.xscc.plist'
[ -f "$plist" ] && [ ! -L "$plist" ] || exit 8
/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' "$plist" | /usr/bin/grep -qx '/usr/local/libexec/xscc' || exit 8
if launchctl print system/org.sarmg.xscc >/dev/null 2>&1; then
  launchctl bootout system/org.sarmg.xscc
fi
if launchctl print system/org.sarmg.xscc >/dev/null 2>&1; then exit 9; fi
launchctl disable system/org.sarmg.xscc
[ "$(readlink /usr/local/bin/xscc)" = /usr/local/libexec/xscc ] || exit 8
rm /usr/local/bin/xscc /usr/local/libexec/xscc "$plist"
echo 'Identity, execution journal, service account and logs retained. Revoke the device in Manager before retirement.'
