#!/usr/bin/env bash

sign_ios_bundle() {
    local bundle="$1" profile="$2" identifier="$3" signing_work="$4"
    local assigned team entitlements="$signing_work/entitlements.plist"
    security cms -D -i "$profile" > "$signing_work/profile.plist"
    team="$(plutil -extract TeamIdentifier.0 raw -o - "$signing_work/profile.plist")"
    assigned="$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:application-identifier' "$signing_work/profile.plist")"
    if [[ ! "$team" =~ ^[A-Z0-9]{10}$ || "$assigned" != "$team.$identifier" ]]; then
        printf 'Provisioning profile does not match %s.\n' "$identifier" >&2
        return 1
    fi
    if ! /usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.developer.networking.networkextension' "$signing_work/profile.plist" |
        grep -Eq '^[[:space:]]+packet-tunnel-provider[[:space:]]*$'; then
        printf 'Provisioning profile for %s lacks the packet-tunnel entitlement.\n' "$identifier" >&2
        return 1
    fi

    cat > "$entitlements" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>application-identifier</key><string>$assigned</string>
  <key>com.apple.developer.team-identifier</key><string>$team</string>
  <key>keychain-access-groups</key><array><string>$assigned</string></array>
  <key>com.apple.developer.networking.networkextension</key>
  <array><string>packet-tunnel-provider</string></array>
</dict></plist>
PLIST
    install -m 0644 -- "$profile" "$bundle/embedded.mobileprovision"
    codesign --force --sign "$BLAZER_IOS_SIGN_IDENTITY" --entitlements "$entitlements" "$bundle"
}

package_ios_tunnel() (
    local app="$1" version="$2" build="$3"
    local tunnel="$app/PlugIns/BlazerTunnel.appex"
    local signing_work
    signing_work="$(mktemp -d "${TMPDIR:-/tmp}/blazer-signing.XXXXXXXX")"
    trap 'rm -f -- "$signing_work/profile.plist" "$signing_work/entitlements.plist"; rmdir -- "$signing_work"' EXIT

    mkdir -p -- "$tunnel"
    source scripts/build-apple-observer.sh
    build_apple_observer aarch64-apple-ios "$BLAZER_IOS_CORE_HEADER"
    xcrun --sdk iphoneos swiftc -swift-version 5 -parse-as-library -emit-executable -O \
        -target "arm64-apple-ios$IPHONEOS_DEPLOYMENT_TARGET" -sdk "$SDKROOT" \
        -import-objc-header "$BLAZER_OBSERVER_HEADER" \
        -application-extension -Xlinker -e -Xlinker _NSExtensionMain \
        src/platform/apple/tunnel/*.swift "$BLAZER_IOS_CORE_LIBRARY" "$BLAZER_OBSERVER_LIBRARY" \
        -Xlinker -u -Xlinker _blazecore_get_capabilities \
        -Xlinker -u -Xlinker _blazecore_select_outbound \
        -framework Foundation -framework Network -framework NetworkExtension \
        -framework Security -framework SystemConfiguration -lresolv -liconv \
        -o "$tunnel/BlazerTunnel"
    install -m 0644 -- src/platform/ios/tunnel/Info.plist "$tunnel/Info.plist"
    plutil -insert CFBundleShortVersionString -string "$version" "$tunnel/Info.plist"
    plutil -insert CFBundleVersion -string "$build" "$tunnel/Info.plist"
    plutil -insert MinimumOSVersion -string "$IPHONEOS_DEPLOYMENT_TARGET" "$tunnel/Info.plist"
    plutil -lint "$tunnel/Info.plist" >/dev/null
    sign_ios_bundle "$tunnel" "$BLAZER_IOS_TUNNEL_PROFILE" com.kobaproduction.blazer.tunnel "$signing_work"
    sign_ios_bundle "$app" "$BLAZER_IOS_APP_PROFILE" com.kobaproduction.blazer "$signing_work"
    codesign --verify --deep --strict "$app"
)
