package com.kobaproduction.blazer;

/** Included only by scripts/build-android.ps1 -Acceptance. */
final class AcceptanceBridge {
    static native String execute(String operation, String payload);
    private AcceptanceBridge() {}
}
