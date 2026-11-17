package com.kobaproduction.blazer;

import java.lang.ref.WeakReference;

/** JNI boundary for the rendering surface. NativeSession owns the process-wide client. */
final class EguiNative {
    private static volatile WeakReference<EguiSurface> surface = new WeakReference<>(null);

    static native long create();
    static native void destroy(long handle);
    static native void surfaceReset(long handle);
    static native byte[] render(long handle, int width, int height, float density, long nowNanos);
    static native void touch(long handle, int action, int pointerId, float x, float y);
    static native void text(long handle, String text);
    static native void key(long handle, int code, boolean down, int modifiers);
    static native void paste(long handle, String text);
    static native void ime(long handle, int phase, String text);
    static native void deleteSurrounding(long handle, int before, int after);
    static native void importedQr(long handle, String text);

    static void attach(EguiSurface view) {
        surface = new WeakReference<>(view);
    }

    static void detach(EguiSurface view) {
        if (surface.get() == view) surface = new WeakReference<>(null);
    }

    static void requestRender(long handle, long delayMillis) {
        EguiSurface view = find(handle);
        if (view != null) view.scheduleRender(delayMillis);
    }

    static boolean requestVpnConsent(long handle) {
        EguiSurface view = find(handle);
        return view != null && view.postToActivity(EguiActivity::requestVpnConsent);
    }

    static boolean pickQrImage(long handle, String title) {
        EguiSurface view = find(handle);
        return view != null && view.postToActivity(activity -> activity.pickQrImage(title));
    }

    static boolean captureQrImage(long handle) {
        return false;
    }

    private static EguiSurface find(long handle) {
        EguiSurface view = surface.get();
        return view != null && view.handle() == handle ? view : null;
    }

    private EguiNative() {}
}
