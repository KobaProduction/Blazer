#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
    printf '%s\n' 'Use a Mac with Xcode to build Blazer for Apple platforms.' >&2
    exit 1
fi

case "$(uname -m)" in
    arm64) architecture=aarch64 ;;
    x86_64) architecture=x86_64 ;;
    *) printf '%s\n' 'Unsupported Mac architecture.' >&2; exit 1 ;;
esac

lane="${1:-}"
case "$lane" in
    macos|macos-tunnel)
        sdk=macosx
        package=blazer-macos
        executable=blazer-macos
        target="${architecture}-apple-darwin"
        ;;
    ios-simulator)
        sdk=iphonesimulator
        package=blazer-ios
        executable=blazer-ios
        target="${architecture}-apple-ios"
        if [[ "$architecture" == aarch64 ]]; then target+=-sim; fi
        ;;
    ios-device)
        sdk=iphoneos
        package=blazer-ios
        executable=blazer-ios
        target=aarch64-apple-ios
        ;;
    *) printf '%s\n' 'Usage: build-apple.sh macos|macos-tunnel|ios-simulator|ios-device [--tagged-version]' >&2; exit 2 ;;
esac

release_version() {
    local tags status candidate stable=''
    tags="$(git tag --points-at HEAD --list 'v*')" || return 1
    status="$(git status --porcelain --untracked-files=normal)" || return 1
    local pattern='^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
    while IFS= read -r candidate; do
        if [[ "$candidate" =~ $pattern ]]; then
            [[ -z "$stable" ]] || return 1
            stable="$candidate"
        fi
    done <<< "$tags"
    [[ -n "$stable" && -z "$status" ]] || return 1
    printf '%s\n' "${stable#v}"
}

cd -- "$(dirname -- "$0")/.."
unset BLAZER_RELEASE_VERSION BLAZER_RELEASE_USER_AGENT
case "${2:-}" in
    '')
        identity="$(rustup run 1.95.0 cargo pkgid --locked -p "$package")"
        version="${identity##*#}"
        version="${version##*@}"
        version="${version%.*}"
        ;;
    --tagged-version)
        version="$(release_version)" || { printf '%s\n' 'A tagged build needs a clean checkout and one stable vX.Y tag at HEAD.' >&2; exit 1; }
        export BLAZER_RELEASE_VERSION="$version" BLAZER_RELEASE_USER_AGENT="Blazer/$version"
        ;;
    *) printf '%s\n' 'Unknown version option.' >&2; exit 2 ;;
esac
version_pattern='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
build_pattern='^[1-9][0-9]{0,3}(\.(0|[1-9][0-9]?)){0,2}$'
build_number="${BLAZER_APPLE_BUILD_NUMBER:-1}"
if [[ ! "$version" =~ $version_pattern || ! "$build_number" =~ $build_pattern ]]; then
    printf '%s\n' 'Invalid Apple marketing version or build number.' >&2
    exit 1
fi

export SDKROOT
SDKROOT="$(xcrun --sdk "$sdk" --show-sdk-path)"
target_dir="$PWD/target"
features=()
if [[ "$sdk" == iphoneos ]]; then
    : "${BLAZER_IOS_CORE_LIBRARY:?Set the verified iOS ARM64 BlazeCore static library path.}"
    : "${BLAZER_IOS_CORE_HEADER:?Set the matching BlazeCore C header path.}"
    : "${BLAZER_IOS_CORE_SHA256:?Set the verified lowercase SHA-256 of the library.}"
    : "${BLAZER_IOS_SIGN_IDENTITY:?Set an Apple signing identity.}"
    : "${BLAZER_IOS_APP_PROFILE:?Set the app provisioning profile.}"
    : "${BLAZER_IOS_TUNNEL_PROFILE:?Set the packet-tunnel provisioning profile.}"
    read -r digest _ < <(shasum -a 256 "$BLAZER_IOS_CORE_LIBRARY")
    if [[ "$digest" != "$BLAZER_IOS_CORE_SHA256" ]]; then
        printf '%s\n' 'The iOS BlazeCore library does not match the verified digest.' >&2
        exit 1
    fi
    export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.0}"
    features=(--features packet-tunnel)
elif [[ "$lane" == macos-tunnel ]]; then
    source scripts/package-macos-tunnel.sh
    check_macos_tunnel_inputs
    export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"
    features=(--features packet-tunnel)
fi
rustup run 1.95.0 cargo build --locked --target-dir "$target_dir" --target "$target" -p "$package" ${features[@]+"${features[@]}"}
binary="$target_dir/$target/debug/$executable"

if [[ "$sdk" == macosx ]]; then
    bundle="$target_dir/apple/$target/Blazer.app"
    if [[ "$lane" == macos-tunnel ]]; then
        bundle="$target_dir/apple/$target/tunnel/Blazer.app"
    fi
    contents="$bundle/Contents"
    mkdir -p -- "$contents/MacOS"
    install -m 0755 -- "$binary" "$contents/MacOS/blazer"
    if [[ -n "${BLAZER_MACOS_CORE_BIN:-}" ]]; then
        read -r checksum _ < <(shasum -a 256 "$BLAZER_MACOS_CORE_BIN")
        if [[ "$checksum" != "${BLAZER_MACOS_CORE_SHA256:-}" || -z "${BLAZER_MACOS_CORE_VERSION:-}" ]]; then
            printf '%s\n' 'Provide the verified lowercase SHA-256 and exact version of the macOS BlazeCore executable.' >&2
            exit 1
        fi
        install -m 0755 -- "$BLAZER_MACOS_CORE_BIN" "$contents/MacOS/blazecore"
    else
        rm -f -- "$contents/MacOS/blazecore"
    fi
    cat > "$contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>com.kobaproduction.blazer</string>
  <key>CFBundleExecutable</key>
  <string>blazer</string>
  <key>CFBundleName</key>
  <string>Blazer</string>
  <key>CFBundleDisplayName</key>
  <string>Blazer</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
</dict>
</plist>
PLIST
    plutil -insert CFBundleShortVersionString -string "$version.0" "$contents/Info.plist"
    plutil -insert CFBundleVersion -string "$build_number" "$contents/Info.plist"
    plutil -lint "$contents/Info.plist" >/dev/null
    if [[ "$lane" == macos-tunnel ]]; then
        package_macos_tunnel "$bundle" "$version.0" "$build_number" "$(uname -m)"
    else
        codesign --force --sign "${BLAZER_MACOS_SIGN_IDENTITY:--}" "$bundle"
    fi
    printf '%s\n' "$bundle"
    exit 0
fi

bundle="$target_dir/apple/$target/Blazer.app"
mkdir -p -- "$bundle"
install -m 0755 -- "$binary" "$bundle/blazer"
cat > "$bundle/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>com.kobaproduction.blazer</string>
  <key>CFBundleExecutable</key>
  <string>blazer</string>
  <key>CFBundleName</key>
  <string>Blazer</string>
  <key>CFBundleDisplayName</key>
  <string>Blazer</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleSupportedPlatforms</key>
  <array><string>iPhoneSimulator</string></array>
  <key>LSRequiresIPhoneOS</key>
  <true/>
  <key>UILaunchScreen</key>
  <dict/>
  <key>UISupportedInterfaceOrientations</key>
  <array>
    <string>UIInterfaceOrientationPortrait</string>
    <string>UIInterfaceOrientationLandscapeLeft</string>
    <string>UIInterfaceOrientationLandscapeRight</string>
  </array>
  <key>UIDeviceFamily</key>
  <array><integer>1</integer><integer>2</integer></array>
</dict>
</plist>
PLIST
plutil -insert CFBundleShortVersionString -string "$version.0" "$bundle/Info.plist"
plutil -insert CFBundleVersion -string "$build_number" "$bundle/Info.plist"
if [[ "$sdk" == iphoneos ]]; then
    plutil -replace CFBundleSupportedPlatforms.0 -string iPhoneOS "$bundle/Info.plist"
    plutil -insert MinimumOSVersion -string "$IPHONEOS_DEPLOYMENT_TARGET" "$bundle/Info.plist"
    source scripts/package-ios-tunnel.sh
    package_ios_tunnel "$bundle" "$version.0" "$build_number"
else
    codesign --force --sign - "$bundle"
fi
plutil -lint "$bundle/Info.plist" >/dev/null
printf '%s\n' "$bundle"
