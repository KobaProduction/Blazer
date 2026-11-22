#!/usr/bin/env bash

check_macos_tunnel_inputs() {
    : "${BLAZER_MACOS_TUNNEL_CORE_LIBRARY:?Set the BlazeCore macOS static library with feature ffi.}"
    : "${BLAZER_MACOS_TUNNEL_CORE_HEADER:?Set the matching BlazeCore C header.}"
    : "${BLAZER_MACOS_TUNNEL_CORE_SHA256:?Set the verified static library SHA-256.}"
    : "${BLAZER_MACOS_SIGN_IDENTITY:?Set a Developer ID Application signing identity.}"
    : "${BLAZER_MACOS_APP_PROFILE:?Set the Developer ID profile for Blazer.}"
    : "${BLAZER_MACOS_TUNNEL_PROFILE:?Set the Developer ID profile for the system extension.}"
    local digest
    read -r digest _ < <(shasum -a 256 "$BLAZER_MACOS_TUNNEL_CORE_LIBRARY")
    if [[ "$digest" != "$BLAZER_MACOS_TUNNEL_CORE_SHA256" ]]; then
        printf 'The macOS BlazeCore static library does not match the verified digest.\n' >&2
        return 1
    fi
}

macos_profile_team() {
    local profile="$1" bundle_id="$2" decoded="$3" team assigned
    security cms -D -i "$profile" > "$decoded"
    team="$(plutil -extract TeamIdentifier.0 raw -o - "$decoded")"
    assigned="$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.application-identifier' "$decoded")"
    if [[ ! "$team" =~ ^[A-Z0-9]{10}$ || "$assigned" != "$team.$bundle_id" ]]; then
        printf 'Provisioning profile does not match %s.\n' "$bundle_id" >&2
        return 1
    fi
    if ! /usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.developer.networking.networkextension' "$decoded" |
        grep -Eq '^[[:space:]]+packet-tunnel-provider-systemextension[[:space:]]*$'; then
        printf 'Provisioning profile for %s lacks the system packet-tunnel entitlement.\n' "$bundle_id" >&2
        return 1
    fi
    printf '%s\n' "$team"
}

sign_macos_tunnel_bundle() {
    local contents="$1/Contents" profile="$2" bundle_id="$3" team="$4" role="$5" entitlements="$6"
    local app_only='<key>com.apple.developer.system-extension.install</key><true/>'
    local tunnel_only="<key>com.apple.security.application-groups</key><array><string>$team.com.kobaproduction.blazer</string></array>"
    local extra="$app_only"
    if [[ "$role" == tunnel ]]; then extra="$tunnel_only"; fi
    cat > "$entitlements" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>com.apple.application-identifier</key><string>$team.$bundle_id</string>
    <key>com.apple.developer.team-identifier</key><string>$team</string>
    <key>com.apple.developer.networking.networkextension</key>
    <array><string>packet-tunnel-provider-systemextension</string></array>
    $extra
</dict></plist>
PLIST
    install -m 0644 -- "$profile" "$contents/embedded.provisionprofile"
    codesign --force --sign "$BLAZER_MACOS_SIGN_IDENTITY" --options runtime --timestamp \
        --entitlements "$entitlements" "$1"
}

package_macos_tunnel() (
    local app="$1" version="$2" build="$3" architecture="$4"
    local app_id=com.kobaproduction.blazer tunnel_id=com.kobaproduction.blazer.packet-tunnel
    local tunnel="$app/Contents/Library/SystemExtensions/$tunnel_id.systemextension"
    local contents="$tunnel/Contents" work team tunnel_team
    work="$(mktemp -d "${TMPDIR:-/tmp}/blazer-macos-signing.XXXXXXXX")"
    trap 'rm -f -- "$work/profile.plist" "$work/entitlements.plist"; rmdir -- "$work"' EXIT

    team="$(macos_profile_team "$BLAZER_MACOS_APP_PROFILE" "$app_id" "$work/profile.plist")"
    tunnel_team="$(macos_profile_team "$BLAZER_MACOS_TUNNEL_PROFILE" "$tunnel_id" "$work/profile.plist")"
    if [[ "$team" != "$tunnel_team" ]]; then
        printf 'Blazer and its system extension must use the same Apple Team ID.\n' >&2
        exit 1
    fi
    mkdir -p -- "$contents/MacOS"
    source scripts/build-apple-observer.sh
    local observer_arch="$architecture"
    if [[ "$observer_arch" == arm64 ]]; then observer_arch=aarch64; fi
    build_apple_observer "$observer_arch-apple-darwin" "$BLAZER_MACOS_TUNNEL_CORE_HEADER"
    xcrun --sdk macosx swiftc -swift-version 5 -parse-as-library -emit-executable -O \
        -target "$architecture-apple-macosx$MACOSX_DEPLOYMENT_TARGET" -sdk "$SDKROOT" \
        -import-objc-header "$BLAZER_OBSERVER_HEADER" \
        src/platform/apple/tunnel/*.swift src/platform/macos/tunnel/SystemExtension.swift \
        "$BLAZER_MACOS_TUNNEL_CORE_LIBRARY" "$BLAZER_OBSERVER_LIBRARY" \
        -Xlinker -u -Xlinker _blazecore_get_capabilities \
        -Xlinker -u -Xlinker _blazecore_select_outbound \
        -framework Foundation -framework Network -framework NetworkExtension \
        -framework Security -framework SystemConfiguration -lresolv -liconv \
        -o "$contents/MacOS/$tunnel_id"
    install -m 0644 -- src/platform/macos/tunnel/Info.plist "$contents/Info.plist"
    plutil -insert CFBundleShortVersionString -string "$version" "$contents/Info.plist"
    plutil -insert CFBundleVersion -string "$build" "$contents/Info.plist"
    plutil -insert LSMinimumSystemVersion -string "$MACOSX_DEPLOYMENT_TARGET" "$contents/Info.plist"
    plutil -insert NetworkExtension.NEMachServiceName -string "$team.$app_id.packet-tunnel" "$contents/Info.plist"
    plutil -insert NSSystemExtensionUsageDescription -string 'Blazer manages its VPN connections through this extension.' "$app/Contents/Info.plist"
    plutil -insert LSMinimumSystemVersion -string "$MACOSX_DEPLOYMENT_TARGET" "$app/Contents/Info.plist"
    plutil -lint "$contents/Info.plist" "$app/Contents/Info.plist" >/dev/null
    sign_macos_tunnel_bundle "$tunnel" "$BLAZER_MACOS_TUNNEL_PROFILE" "$tunnel_id" "$team" tunnel "$work/entitlements.plist"
    sign_macos_tunnel_bundle "$app" "$BLAZER_MACOS_APP_PROFILE" "$app_id" "$team" app "$work/entitlements.plist"
    codesign --verify --deep --strict "$app"
)
