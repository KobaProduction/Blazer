#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
    printf '%s\n' 'Use a Mac with Xcode to build the Apple previews.' >&2
    exit 1
fi

case "$(uname -m)" in
    arm64) architecture=aarch64 ;;
    x86_64) architecture=x86_64 ;;
    *) printf '%s\n' 'Unsupported Mac architecture.' >&2; exit 1 ;;
esac

case "${1:-}" in
    macos)
        sdk=macosx
        package=blazer-macos-preview
        target="${architecture}-apple-darwin"
        ;;
    ios-simulator)
        sdk=iphonesimulator
        package=blazer-ios-preview
        target="${architecture}-apple-ios"
        if [[ "$architecture" == aarch64 ]]; then target+=-sim; fi
        ;;
    *) printf '%s\n' 'Usage: build-apple-preview.sh macos|ios-simulator [--tagged-version]' >&2; exit 2 ;;
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
rustup run 1.95.0 cargo build --locked --target-dir "$target_dir" --target "$target" -p "$package"
binary="$target_dir/$target/debug/$package"

if [[ "$sdk" == macosx ]]; then
    printf '%s\n' "$binary"
    exit 0
fi

bundle="$target_dir/apple-preview/$target/BlazerPreview.app"
mkdir -p -- "$bundle"
install -m 0755 -- "$binary" "$bundle/blazer-ios-preview"
cat > "$bundle/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>com.kobaproduction.blazer.preview</string>
  <key>CFBundleExecutable</key>
  <string>blazer-ios-preview</string>
  <key>CFBundleName</key>
  <string>Blazer Preview</string>
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
plutil -lint "$bundle/Info.plist" >/dev/null
codesign --force --sign - "$bundle"
printf '%s\n' "$bundle"
