package com.shoesproxy;

import android.net.VpnService;

public final class ShoesNative {
    private static final boolean AVAILABLE = load();

    private static boolean load() {
        try {
            System.loadLibrary("shoes");
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

    private ShoesNative() {}
}
