package com.kobaproduction.vpnclient;

import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import com.shoesproxy.ShoesNative;
import java.io.File;
import java.lang.ref.WeakReference;
import java.util.concurrent.atomic.AtomicBoolean;

final class NativeSession {
    static {
        System.loadLibrary("vpn_android");
    }

    private static final Handler UI = new Handler(Looper.getMainLooper());
    private static final AtomicBoolean POSTED = new AtomicBoolean();
    private static WeakReference<MainActivity> observer = new WeakReference<>(null);

    private static native boolean open(String dataDir, boolean shoesPackaged);
    static native String snapshot();
    static native boolean dispatch(String request);
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
        File shoes = new File(context.getApplicationInfo().nativeLibraryDir, System.mapLibraryName("shoes"));
        if (!open(context.getNoBackupFilesDir().getAbsolutePath(), shoes.isFile())) return false;
        SubscriptionRefreshJob.attach(context);
        if (foreground) setForeground(true);
        onNativeUpdate();
        return true;
    }

    static void observe(MainActivity activity) {
        observer = new WeakReference<>(activity);
    }

    static void detach(MainActivity activity) {
        if (observer.get() == activity) {
            observer.clear();
            setForeground(false);
        }
    }

    /** Called by the shared Rust backend after publishing a snapshot. */
    static void onNativeUpdate() {
        if (POSTED.compareAndSet(false, true)) UI.post(NativeSession::publish);
    }

    private static void publish() {
        POSTED.set(false);
        ClientVpnService.onNativeUpdate();
        SubscriptionRefreshJob.onNativeUpdate();
        MainActivity activity = observer.get();
        if (activity != null) activity.refresh();
    }

    /** Called on the backend worker, never on the Activity rendering thread. */
    static byte[] sealCatalog(byte[] input, boolean encrypt) {
        return CatalogSeal.transform(input, encrypt);
    }

    static boolean loadEngine() { return ShoesNative.available(); }
    static int openTunnel() { return ClientVpnService.openTunnel(); }
    static boolean startTunnel(String config) { return ClientVpnService.startTunnel(config); }
    static boolean tunnelRunning() { return ClientVpnService.tunnelRunning(); }
    static boolean tunnelRouted() { return ClientVpnService.tunnelRouted(); }
    static boolean stopTunnel() { return ClientVpnService.stopTunnel(); }

    private NativeSession() {}
}
