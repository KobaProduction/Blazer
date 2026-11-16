package com.kobaproduction.vpnclient;

import android.app.Activity;
import android.app.AlertDialog;
import android.graphics.ImageFormat;
import android.hardware.Camera;
import android.os.Handler;
import android.os.HandlerThread;
import android.view.Gravity;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;
import java.io.IOException;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;

/** A bounded, in-memory scanner. The camera and the decoder both run off the UI thread. */
@SuppressWarnings("deprecation")
final class QrCameraScanner implements SurfaceHolder.Callback {
    private static final int MAX_PREVIEW_PIXELS = 1280 * 720;
    private static final long SCAN_PERIOD_MS = 240;

    private final Activity activity;
    private final Consumer<String> found;
    private final Runnable failed;
    private final HandlerThread thread = new HandlerThread("qr-camera");
    private final AtomicBoolean closed = new AtomicBoolean();
    private final Handler worker;
    private final AlertDialog dialog;
    private final FrameLayout viewport;
    private final SurfaceView preview;
    private final int displayAngle;
    private Camera camera;

    QrCameraScanner(Activity activity, String title, String hint, String cancel,
            Consumer<String> found, Runnable failed) {
        this.activity = activity;
        this.found = found;
        this.failed = failed;
        displayAngle = displayAngle(activity);
        thread.start();
        worker = new Handler(thread.getLooper());

        preview = new SurfaceView(activity);
        preview.getHolder().addCallback(this);
        TextView caption = new TextView(activity);
        caption.setText(hint);
        int padding = (int) (16 * activity.getResources().getDisplayMetrics().density);
        caption.setPadding(padding, padding, padding, padding);
        LinearLayout content = new LinearLayout(activity);
        content.setOrientation(LinearLayout.VERTICAL);
        content.addView(caption);
        viewport = new FrameLayout(activity);
        viewport.setBackgroundColor(android.graphics.Color.BLACK);
        viewport.addView(preview, new FrameLayout.LayoutParams(-1, -1, Gravity.CENTER));
        content.addView(viewport, new LinearLayout.LayoutParams(-1, 16 * padding));
        dialog = new AlertDialog.Builder(activity).setTitle(title).setView(content)
            .setNegativeButton(cancel, null).create();
        dialog.setOnDismissListener(ignored -> release());
        dialog.show();
    }

    @Override public void surfaceCreated(SurfaceHolder holder) {
        worker.post(() -> start(holder));
    }

    @Override public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {}

    @Override public void surfaceDestroyed(SurfaceHolder holder) {
        close();
    }

    void close() {
        release();
        dialog.dismiss();
    }

    private void release() {
        if (!closed.compareAndSet(false, true)) return;
        worker.post(() -> {
            if (camera != null) {
                camera.release();
                camera = null;
            }
            thread.quitSafely();
        });
    }

    private void start(SurfaceHolder holder) {
        if (closed.get()) return;
        try {
            int index = cameraIndex();
            camera = Camera.open(index);
            Camera.Parameters settings = camera.getParameters();
            Camera.Size size = previewSize(settings);
            settings.setPreviewFormat(ImageFormat.NV21);
            settings.setPreviewSize(size.width, size.height);
            List<String> focusModes = settings.getSupportedFocusModes();
            if (focusModes != null && focusModes.contains(Camera.Parameters.FOCUS_MODE_CONTINUOUS_PICTURE)) {
                settings.setFocusMode(Camera.Parameters.FOCUS_MODE_CONTINUOUS_PICTURE);
            }
            camera.setParameters(settings);
            int angle = orientation(index);
            camera.setDisplayOrientation(angle);
            camera.setPreviewDisplay(holder);
            camera.setPreviewCallbackWithBuffer((frame, active) -> scan(frame, size.width, size.height));
            camera.addCallbackBuffer(new byte[size.width * size.height * 3 / 2]);
            camera.startPreview();
            activity.runOnUiThread(() -> fitPreview(size, angle));
        } catch (IOException | RuntimeException failure) {
            activity.runOnUiThread(() -> {
                if (closed.get()) return;
                close();
                failed.run();
            });
        }
    }

    private void fitPreview(Camera.Size size, int angle) {
        if (closed.get()) return;
        boolean rotated = angle % 180 != 0;
        int frameWidth = rotated ? size.height : size.width;
        int frameHeight = rotated ? size.width : size.height;
        float scale = Math.min((float) viewport.getWidth() / frameWidth,
            (float) viewport.getHeight() / frameHeight);
        if (scale > 0) {
            preview.setLayoutParams(new FrameLayout.LayoutParams(
                Math.max(1, Math.round(frameWidth * scale)),
                Math.max(1, Math.round(frameHeight * scale)), Gravity.CENTER));
        }
    }

    private void scan(byte[] frame, int width, int height) {
        if (closed.get() || frame == null) return;
        String result = NativeSession.decodeQrFrame(frame, width, height);
        if (result.isEmpty()) {
            worker.postDelayed(() -> {
                if (!closed.get() && camera != null) camera.addCallbackBuffer(frame);
            }, SCAN_PERIOD_MS);
            return;
        }
        activity.runOnUiThread(() -> {
            if (closed.get()) return;
            close();
            found.accept(result);
        });
    }

    private static int cameraIndex() {
        int count = Camera.getNumberOfCameras();
        Camera.CameraInfo info = new Camera.CameraInfo();
        for (int index = 0; index < count; index++) {
            Camera.getCameraInfo(index, info);
            if (info.facing == Camera.CameraInfo.CAMERA_FACING_BACK) return index;
        }
        return 0;
    }

    private static Camera.Size previewSize(Camera.Parameters settings) {
        Camera.Size chosen = null;
        List<Camera.Size> sizes = settings.getSupportedPreviewSizes();
        if (sizes == null) sizes = Collections.singletonList(settings.getPreviewSize());
        for (Camera.Size size : sizes) {
            int pixels = size.width * size.height;
            if (pixels <= MAX_PREVIEW_PIXELS && (chosen == null || pixels > chosen.width * chosen.height)) {
                chosen = size;
            }
        }
        if (chosen == null) throw new IllegalStateException("No bounded preview size");
        return chosen;
    }

    private int orientation(int index) {
        Camera.CameraInfo info = new Camera.CameraInfo();
        Camera.getCameraInfo(index, info);
        if (info.facing == Camera.CameraInfo.CAMERA_FACING_FRONT) {
            return (720 - info.orientation - displayAngle) % 360;
        }
        return (360 + info.orientation - displayAngle) % 360;
    }

    private static int displayAngle(Activity activity) {
        return switch (activity.getWindowManager().getDefaultDisplay().getRotation()) {
            case Surface.ROTATION_90 -> 90;
            case Surface.ROTATION_180 -> 180;
            case Surface.ROTATION_270 -> 270;
            default -> 0;
        };
    }
}
