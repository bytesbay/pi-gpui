#!/bin/sh
# Notarizes a Developer ID signed app (script/bundle.sh with SIGN_IDENTITY)
# and staples the ticket, so Gatekeeper opens it without a warning, offline too.
#   script/notarize.sh "target/release/Pi GPUI.app"
# Credentials, one of:
#   NOTARY_PROFILE      a profile saved with `xcrun notarytool store-credentials`
#   APPLE_API_KEY_PATH, APPLE_API_KEY_ID, APPLE_API_ISSUER_ID
#                       an App Store Connect API key (.p8), as CI uses
set -eu
app=${1:-"target/release/Pi GPUI.app"}
if [ -n "${NOTARY_PROFILE:-}" ]; then
    set -- --keychain-profile "$NOTARY_PROFILE"
elif [ -n "${APPLE_API_KEY_PATH:-}" ]; then
    set -- --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID"
else
    echo "Set NOTARY_PROFILE or APPLE_API_KEY_PATH/APPLE_API_KEY_ID/APPLE_API_ISSUER_ID." >&2
    exit 1
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
ditto -c -k --sequesterRsrc --keepParent "$app" "$work/notarize.zip"
if ! xcrun notarytool submit "$work/notarize.zip" "$@" --wait --output-format json > "$work/result.json"; then
    cat "$work/result.json" >&2
    exit 1
fi
cat "$work/result.json"
status=$(plutil -extract status raw "$work/result.json")
if [ "$status" != "Accepted" ]; then
    id=$(plutil -extract id raw "$work/result.json")
    xcrun notarytool log "$id" "$@" >&2 || true
    exit 1
fi
# Stapling fetches the ticket from Apple's CDN, which sometimes drops the
# connection right after notarization; retry before failing the release.
for attempt in 1 2 3 4 5; do
    xcrun stapler staple "$app" && break
    [ "$attempt" = 5 ] && exit 1
    sleep $((attempt * 10))
done
spctl --assess --type execute --verbose=2 "$app"
