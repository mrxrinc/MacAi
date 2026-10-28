#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

if [ -z "${MACAI_SIGNING_IDENTITY:-}" ] || [ -z "${MACAI_NOTARY_PROFILE:-}" ]; then
    printf '%s\n' \
        'A public release requires a Developer ID Application certificate and notarization credentials.' \
        'Set MACAI_SIGNING_IDENTITY and MACAI_NOTARY_PROFILE; see README.md.' >&2
    exit 1
fi
case "$MACAI_SIGNING_IDENTITY" in
    "Developer ID Application: "*) ;;
    *)
        printf '%s\n' 'Use a Developer ID Application identity, not Apple Development or Apple Distribution.' >&2
        exit 1
        ;;
esac
identities=$(security find-identity -v -p codesigning)
if ! printf '%s\n' "$identities" \
    | grep -F -- "\"$MACAI_SIGNING_IDENTITY\"" \
    | grep -v 'CSSMERR_' >/dev/null; then
    printf '%s\n' 'The requested valid Developer ID Application identity was not found in your keychain.' >&2
    exit 1
fi

sh scripts/build.sh
app="$PWD/dist/MacAi.app"
codesign --force --options runtime --timestamp --sign "$MACAI_SIGNING_IDENTITY" "$app"
codesign --verify --strict --verbose=2 "$app"

mkdir -p target/notarization
work=$(mktemp -d "$PWD/target/notarization/run.XXXXXX")
trap 'rm -f "$work/upload.zip" "$work/distribution.zip"' EXIT
ditto -c -k --sequesterRsrc --keepParent "$app" "$work/upload.zip"

if ! xcrun notarytool submit "$work/upload.zip" \
    --keychain-profile "$MACAI_NOTARY_PROFILE" --wait --output-format json > "$work/result.json"; then
    printf 'Notarization failed. No new distribution archive was published. Details: %s/result.json\n' "$work" >&2
    exit 1
fi
status=$(plutil -extract status raw -o - "$work/result.json")
if [ "$status" != Accepted ]; then
    submission=$(plutil -extract id raw -o - "$work/result.json")
    if ! xcrun notarytool log "$submission" --keychain-profile "$MACAI_NOTARY_PROFILE" "$work/notary-log.json"; then
        printf '%s\n' 'The notarization diagnostic log could not be downloaded.' >&2
    fi
    printf 'Apple did not accept the app (%s). No new distribution archive was published. See %s\n' "$status" "$work" >&2
    exit 1
fi

xcrun stapler staple "$app"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=4 "$app"
ditto -c -k --sequesterRsrc --keepParent "$app" "$work/distribution.zip"
mv -f "$work/distribution.zip" dist/MacAi.zip
printf '\nDeveloper ID signed, notarized, stapled, and Gatekeeper-verified release:\n%s/dist/MacAi.zip\n' "$PWD"
