#!/bin/bash
set -euo pipefail
# Hosted images make Homebrew directories runner-writable. This setup changes
# only disposable CI hosts, after proving that the product refuses unsafe roots.
[[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_ENVIRONMENT:-} == github-hosted && $(uname -s) == Darwin ]]
[[ $# == 1 && $1 == /* && -x $1 ]]
unsafe=0
for directory in /usr/local /usr/local/bin /usr/local/libexec; do
  [[ ! -L $directory ]]
  if [[ -e $directory ]]; then
    [[ -d $directory ]]
    mode=$(stat -f %Lp "$directory")
    if [[ $(stat -f %u "$directory") != 0 ]] || (( (8#$mode & 0022) != 0 )); then unsafe=1; fi
  fi
done
if [[ $unsafe == 1 ]]; then
  if sudo sh deploy/macos/install-macos.sh "$1"; then
    echo 'Installer accepted unsafe CI roots' >&2; exit 1
  fi
  sudo test ! -e '/Library/Application Support/sunshine-client'
  sudo test ! -e /Library/LaunchDaemons/org.sarmg.sunshine-client.plist
  ! dscl . -read /Users/_sunshineclient >/dev/null 2>&1
fi
for directory in /usr/local /usr/local/bin /usr/local/libexec; do
  sudo install -d -m 0755 -o root -g wheel "$directory"
  sudo chmod -N "$directory"
  sudo chown root:wheel "$directory"
  sudo chmod 0755 "$directory"
done
