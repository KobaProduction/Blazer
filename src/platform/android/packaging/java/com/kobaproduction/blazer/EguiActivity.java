package com.kobaproduction.blazer;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.res.Configuration;
import android.graphics.Insets;
import android.net.Uri;
import android.net.VpnService;
import android.os.Build;
import android.os.Bundle;
import android.os.PersistableBundle;
import android.view.Gravity;
import android.view.WindowInsets;
import android.widget.FrameLayout;
import android.widget.TextView;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import org.json.JSONException;
import org.json.JSONObject;

/** Internal Android shell for the shared egui screens. */
public final class EguiActivity extends Activity {
    private static final int VPN_CONSENT = 201;
    private static final int QR_FILE = 202;
    private static final int MAX_IMAGE_BYTES = 32 * 1024 * 1024;
    private static final int MAX_TEXT_UNITS = 1024 * 1024;

    private FrameLayout root;
    private EguiSurface surface;
    private boolean consentPending;
    private boolean pickingQr;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        if (state != null) {
            consentPending = state.getBoolean("vpnConsent");
            pickingQr = state.getBoolean("qrPicker");
        }
        root = new FrameLayout(this);
        root.setBackgroundResource(R.color.background);
        configureInsets();
        setContentView(root);
    }

    private void configureInsets() {
        if (Build.VERSION.SDK_INT < 30) {
            root.setFitsSystemWindows(true);
            return;
        }
        getWindow().setDecorFitsSystemWindows(false);
        root.setOnApplyWindowInsetsListener((view, insets) -> {
            Insets safe = insets.getInsets(WindowInsets.Type.systemBars()
                | WindowInsets.Type.displayCutout() | WindowInsets.Type.ime());
            view.setPadding(safe.left, safe.top, safe.right, safe.bottom);
            return WindowInsets.CONSUMED;
        });
    }

    @Override protected void onStart() {
        super.onStart();
        NativeSession.observe(this);
        if (!NativeSession.openSession(this, true)) {
            showFailure("Blazer session could not be opened");
            return;
        }
        if (surface == null) {
            long handle = EguiNative.create();
            if (handle == 0) {
                showFailure("Blazer renderer could not be opened");
                return;
            }
            surface = new EguiSurface(this, handle);
            EguiNative.attach(surface);
            root.removeAllViews();
            root.addView(surface, new FrameLayout.LayoutParams(-1, -1));
        }
        surface.resumeDrawing();
    }

    @Override protected void onStop() {
        if (surface != null) surface.pauseDrawing();
        NativeSession.detach(this);
        super.onStop();
    }

    @Override protected void onDestroy() {
        if (surface != null) surface.dispose();
        super.onDestroy();
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        state.putBoolean("vpnConsent", consentPending);
        state.putBoolean("qrPicker", pickingQr);
        super.onSaveInstanceState(state);
    }

    @Override public void onConfigurationChanged(Configuration configuration) {
        super.onConfigurationChanged(configuration);
        root.requestApplyInsets();
        if (surface != null) surface.scheduleRender(0);
    }

    void showFailure(String message) {
        TextView label = new TextView(this);
        label.setText(message);
        label.setGravity(Gravity.CENTER);
        label.setTextColor(getColor(R.color.foreground));
        root.removeAllViews();
        root.addView(label, new FrameLayout.LayoutParams(-1, -1));
    }

    void requestVpnConsent() {
        if (consentPending) return;
        Intent consent = VpnService.prepare(this);
        if (consent == null) {
            ClientVpnService.request(this);
        } else {
            consentPending = true;
            startActivityForResult(consent, VPN_CONSENT);
        }
    }

    void pickQrImage(String title) {
        if (pickingQr) return;
        Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
            .setType("image/*").putExtra(Intent.EXTRA_TITLE, title);
        try {
            startActivityForResult(picker, QR_FILE);
            pickingQr = true;
        } catch (ActivityNotFoundException failure) {
            deliverQr(null);
        }
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == VPN_CONSENT) {
            consentPending = false;
            if (result == RESULT_OK) ClientVpnService.request(this);
        }
        if (request == QR_FILE) {
            pickingQr = false;
            Uri uri = result == RESULT_OK && data != null ? data.getData() : null;
            if (uri == null) deliverQr(null);
            else new Thread(() -> {
                String text = qrText(uri);
                runOnUiThread(() -> deliverQr(text));
            }, "blazer-qr").start();
        }
    }

    private String qrText(Uri uri) {
        try (InputStream source = getContentResolver().openInputStream(uri)) {
            if (source == null) return null;
            ByteArrayOutputStream image = new ByteArrayOutputStream();
            byte[] buffer = new byte[8192];
            for (int count; (count = source.read(buffer)) != -1;) {
                if (count > MAX_IMAGE_BYTES - image.size()) return null;
                image.write(buffer, 0, count);
            }
            JSONObject result = new JSONObject(NativeSession.decodeQrImage(image.toByteArray()));
            return result.optString("text", null);
        } catch (IOException | SecurityException | JSONException failure) {
            return null;
        }
    }

    private void deliverQr(String text) {
        if (surface != null && !isDestroyed() && !isFinishing()) {
            EguiSurface view = surface;
            view.submitInput(() -> EguiNative.importedQr(view.handle(), text));
        }
    }

    void readClipboard() {
        EguiSurface view = surface;
        if (view == null) return;
        ClipboardManager clipboard = getSystemService(ClipboardManager.class);
        ClipData clip = clipboard == null ? null : clipboard.getPrimaryClip();
        CharSequence text = clip == null || clip.getItemCount() == 0 ? null : clip.getItemAt(0).getText();
        String value = text != null && text.length() <= MAX_TEXT_UNITS ? text.toString() : "";
        view.submitInput(() -> EguiNative.paste(view.handle(), value));
    }

    void copyToClipboard(String text) {
        ClipboardManager clipboard = getSystemService(ClipboardManager.class);
        if (clipboard == null) return;
        ClipData clip = ClipData.newPlainText("Blazer", text);
        PersistableBundle extras = new PersistableBundle();
        extras.putBoolean("android.content.extra.IS_SENSITIVE", true);
        clip.getDescription().setExtras(extras);
        clipboard.setPrimaryClip(clip);
    }

    void openUrl(String text) {
        Uri url = Uri.parse(text);
        String scheme = url.getScheme();
        if (url.getHost() == null || !("http".equalsIgnoreCase(scheme)
            || "https".equalsIgnoreCase(scheme))) return;
        try {
            startActivity(new Intent(Intent.ACTION_VIEW, url).addCategory(Intent.CATEGORY_BROWSABLE));
        } catch (ActivityNotFoundException ignored) {
            // No browser is installed.
        }
    }
}
