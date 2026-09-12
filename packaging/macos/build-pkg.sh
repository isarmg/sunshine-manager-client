#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
  echo 'Usage: build-pkg.sh ABSOLUTE_BINARY VERSION ABSOLUTE_OUTPUT' >&2
  exit 2
fi

binary=$1
version=$2
output=$3
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd -P)
plist="$project_dir/deploy/macos/org.sarmg.sunshine-client.plist"

[ "$(uname -s)" = Darwin ] || { echo 'macOS PKG must be built on macOS.' >&2; exit 8; }
[ "$(uname -m)" = arm64 ] || { echo 'Apple Silicon is required.' >&2; exit 8; }
[ -f "$binary" ] && [ ! -L "$binary" ] && [ -x "$binary" ] || exit 8
[ -f "$plist" ] && [ ! -L "$plist" ] || exit 8
[ ! -e "$output" ] && [ ! -L "$output" ] || { echo 'refusing to overwrite the PKG output.' >&2; exit 8; }
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$' || exit 8
/usr/bin/plutil -lint "$plist" >/dev/null
command -v pkgbuild >/dev/null 2>&1 || { echo 'pkgbuild is required.' >&2; exit 8; }

package_version=$(printf '%s\n' "$version" | sed 's/-rc\..*$//')
temporary=$(mktemp -d "${TMPDIR:-/tmp}/sunshine-client-pkg.XXXXXX")
cleanup() {
  rm -rf -- "$temporary"
}
trap cleanup EXIT HUP INT TERM

payload="$temporary/payload"
mkdir -p \
  "$payload/usr/local/libexec" \
  "$payload/usr/local/bin" \
  "$payload/Library/LaunchDaemons"
install -m 0755 "$binary" "$payload/usr/local/libexec/sunshine-client"
ln -s /usr/local/libexec/sunshine-client "$payload/usr/local/bin/sunshine-client"
install -m 0644 "$plist" "$payload/Library/LaunchDaemons/org.sarmg.sunshine-client.plist"

pkgbuild \
  --root "$payload" \
  --scripts "$project_dir/packaging/macos/scripts" \
  --identifier org.sarmg.sunshine-client \
  --version "$package_version" \
  --install-location / \
  "$output"
