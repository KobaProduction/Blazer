package com.blazecore;

import android.net.VpnService;

public final class BlazeCoreNative {
    private static final boolean AVAILABLE = load();

    private static boolean load() {
        try {
            System.loadLibrary("blazecore");
            return "0.3.7".equals(getVersion()) && init("off") == 0;
        } catch (UnsatisfiedLinkError failure) {
            return false;
        }
    }

    public static boolean available() { return AVAILABLE; }
    private static native int init(String logLevel);
    private static native String getVersion();
    public static native long start(String config, VpnService protector);
    public static native void stop(long handle);
    public static native boolean isRunning();

    // Bit 0 is the reviewed core ABI's named-selector control capability.
    public static native long getCapabilities();
    public static native int selectOutbound(long handle, String group, String member);

    public static boolean selectorControlAvailable() {
        if (!AVAILABLE) return false;
        try {
            return (getCapabilities() & 1L) != 0;
        } catch (UnsatisfiedLinkError unavailableAbi) {
            return false;
        }
    }

    private BlazeCoreNative() {}
}
