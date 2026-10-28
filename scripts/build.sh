#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
app="dist/MacAi.app"
contents="$app/Contents"
iconset="target/macos-icon/MacAi.iconset"
mkdir -p "$contents/MacOS" "$contents/Resources/licenses"

swift scripts/make-icon.swift "$iconset" assets/MacAi.png
cargo build --release --locked
iconutil --convert icns --output "$contents/Resources/MacAi.icns" "$iconset"
cp assets/Info.plist "$contents/Info.plist"
cp assets/Credits.html "$contents/Resources/Credits.html"
cp assets/fonts/UFL.txt "$contents/Resources/licenses/UFL.txt"
cp assets/fonts/COPYRIGHT.txt "$contents/Resources/licenses/COPYRIGHT.txt"
plutil -lint "$contents/Info.plist"

trap 'rm -f "$contents/MacOS/MacAi.next"' EXIT
cp target/release/MacAi "$contents/MacOS/MacAi.next"
chmod 755 "$contents/MacOS/MacAi.next"
mv -f "$contents/MacOS/MacAi.next" "$contents/MacOS/MacAi"
codesign --force --sign - "$app"
codesign --verify --strict --verbose=2 "$app"
touch "$app"

printf '\nBuilt local development app: %s/%s\n' "$PWD" "$app"
printf 'This ad-hoc-signed build is NOT notarized for distribution to other Macs.\nUse scripts/release.sh for a Gatekeeper-verified release.\n'
