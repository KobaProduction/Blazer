package com.kobaproduction.blazer;

import android.app.Activity;
import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import com.blazecore.BlazeCoreNative;
import java.io.File;
import java.lang.ref.WeakReference;
import java.util.concurrent.atomic.AtomicBoolean;

final class NativeSession {
    static {
        System.loadLibrary("blazer_android");
    }

    private static final Handler UI = new Handler(Looper.getMainLooper());
    private static final AtomicBoolean POSTED = new AtomicBoolean();
    private static WeakReference<Activity> observer = new WeakReference<>(null);

    private static native boolean open(String dataDir, boolean blazeCorePackaged);
    static native void disconnect();
    static native String decodeQrImage(byte[] image);
    static native String decodeQrFrame(byte[] frame, int width, int height);
    static native long requestTunnelStart();
    // Zero while pending, positive when Rust owns cleanup, negative for a rejected start.
    static native int tunnelStartOutcome(long request);
    private static native void setForeground(boolean visible);
    static native String refreshSchedule();
    static native long startPeriodicRefresh();
    static native void cancelPeriodicRefresh(long request);

    static boolean openSession(Context context, boolean foreground) {
        // The APK manifest enables extraction into Android's installed library directory.
        File blazeCore = new File(context.getApplicationInfo().nativeLibraryDir, System.mapLibraryName("blazecore"));
        if (!open(context.getNoBackupFilesDir().getAbsolutePath(), blazeCore.isFile())) return false;
        SubscriptionRefreshJob.attach(context);
        if (foreground) setForeground(true);
        onNativeUpdate();
        return true;
    }

    static void observe(Activity activity) {
        observer = new WeakReference<>(activity);
    }

    static void detach(Activity activity) {
        if (observer.get() == activity) {
            observer.clear();
            setForeground(false);
        }
    }

    /** Called by the Rust client core after publishing a snapshot. */
    static void onNativeUpdate() {
        if (POSTED.compareAndSet(false, true)) UI.post(NativeSession::publish);
    }

    private static void publish() {
        POSTED.set(false);
        ClientVpnService.onNativeUpdate();
        SubscriptionRefreshJob.onNativeUpdate();
    }

    /** Called on the client worker, never on the Activity rendering thread. */
    static byte[] sealCatalog(byte[] input, boolean encrypt) {
        return CatalogSeal.transform(input, encrypt);
    }

    static boolean loadEngine() { return BlazeCoreNative.available(); }
    static int openTunnel() { return ClientVpnService.openTunnel(); }
    static boolean startTunnel(String config) { return ClientVpnService.startTunnel(config); }
    static boolean tunnelRunning() { return ClientVpnService.tunnelRunning(); }
    static boolean tunnelRouted() { return ClientVpnService.tunnelRouted(); }
    static boolean stopTunnel() { return ClientVpnService.stopTunnel(); }
    static boolean selectorControlAvailable() { return BlazeCoreNative.selectorControlAvailable(); }
    static long tunnelHandle() { return ClientVpnService.tunnelHandle(); }
    static boolean selectorControlReady(long expectedHandle) { return ClientVpnService.selectorControlReady(expectedHandle); }
    static boolean selectOutbound(long expectedHandle, String group, String member) { return ClientVpnService.selectOutbound(expectedHandle, group, member); }

    private NativeSession() {}
}
