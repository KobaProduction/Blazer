#!/usr/bin/env bash

# Sourced by Apple packaging; all paths refer to the existing target directory.
build_apple_observer() {
    local target="$1" core_header="$2"
    core_header="$(cd -- "$(dirname -- "$core_header")" && printf '%s/%s' "$PWD" "$(basename -- "$core_header")")"
    local output="$PWD/target/apple/observer/$target"
    mkdir -p -- "$output"
    if ! grep -q 'blazecore_get_capabilities' "$core_header" ||
       ! grep -q 'blazecore_select_outbound' "$core_header"; then
        printf '%s\n' 'The verified BlazeCore header lacks the required selector-control ABI.' >&2
        return 1
    fi
    rustup run 1.95.0 cargo run --locked -p observer-bridge --features headers \
        --bin observer-header -- "$output/BlazerObserver.h"
    rustup run 1.95.0 cargo build --locked --target "$target" -p observer-bridge
    BLAZER_OBSERVER_LIBRARY="$PWD/target/$target/debug/libobserver_bridge.a"
    BLAZER_OBSERVER_HEADER="$output/BlazerTunnel.h"
    printf '#include "%s"\n#include "%s"\n#include "%s"\n' \
        "$core_header" "$output/BlazerObserver.h" \
        "$PWD/src/platform/apple/tunnel/SelectorCallbacks.h" > "$BLAZER_OBSERVER_HEADER"
}
