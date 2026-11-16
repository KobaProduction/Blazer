package com.kobaproduction.vpnclient;

import android.Manifest;
import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.net.VpnService;
import android.net.Uri;
import android.os.Bundle;
import android.text.InputFilter;
import android.text.InputType;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

public final class MainActivity extends Activity {
    private static final int VPN_CONSENT = 101;
    private static final int QR_FILE = 102;
    private static final int QR_CAMERA_PERMISSION = 103;
    private static final int MAX_TEXT_BYTES = 1024 * 1024;
    private static final int MAX_IMAGE_BYTES = 32 * 1024 * 1024;
    private JSONObject strings = new JSONObject();
    private LinearLayout screen;
    private QrCameraScanner cameraScanner;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.setFitsSystemWindows(true);
        screen = column();
        screen.setPadding(dp(18), dp(18), dp(18), dp(24));
        scroll.addView(screen);
        setContentView(scroll);
    }

    @Override protected void onStart() {
        super.onStart();
        NativeSession.observe(this);
        if (!NativeSession.openSession(this, true)) {
            renderFailure();
            return;
        }
        refresh();
    }

    @Override protected void onStop() {
        if (cameraScanner != null) cameraScanner.close();
        NativeSession.detach(this);
        super.onStop();
    }

    void refresh() {
        try {
            JSONObject snapshot = new JSONObject(NativeSession.snapshot());
            JSONObject labels = snapshot.optJSONObject("labels");
            if (labels == null) {
                renderFailure();
                return;
            }
            strings = labels;
            render(snapshot);
        } catch (JSONException failure) {
            renderFailure();
        }
    }

    private void render(JSONObject snapshot) {
        screen.removeAllViews();
        boolean busy = snapshot.optBoolean("busy", true);
        header(snapshot.optString("language"));
        hero(snapshot, busy);
        section(label("subscriptions"), snapshot.optJSONArray("subscriptions"), busy, true);
        section(label("profiles"), snapshot.optJSONArray("profiles"), busy, false);
    }

    private void header(String language) {
        LinearLayout row = row();
        TextView title = caption(label("title"), 26, R.color.foreground);
        title.setTypeface(null, Typeface.BOLD);
        row.addView(title, new LinearLayout.LayoutParams(0, -2, 1));
        row.addView(button(language, () -> send("toggle_language", null)));
        screen.addView(row);
    }

    private void hero(JSONObject snapshot, boolean busy) {
        LinearLayout panel = card();
        panel.setGravity(Gravity.CENTER_HORIZONTAL);
        String status = busy ? label("busy") : snapshot.optString("status");
        panel.addView(caption(status, 19, R.color.foreground));
        Button connect = button(snapshot.optString("primary_action"),
            () -> primary(snapshot.optBoolean("request_vpn")));
        connect.setEnabled(snapshot.optBoolean("primary_enabled"));
        panel.addView(connect, new LinearLayout.LayoutParams(-1, dp(48)));
        if (!snapshot.optBoolean("mode_available")) panel.addView(caption(label("unavailable"), 13, R.color.muted));
        if (!snapshot.isNull("error")) panel.addView(caption(snapshot.optString("error"), 14, R.color.accent));
        LinearLayout actions = row();
        addAction(actions, label("add_subscription"), busy, () -> form(true));
        addAction(actions, label("import"), busy, () -> form(false));
        panel.addView(actions, new LinearLayout.LayoutParams(-1, -2));
        LinearLayout sources = row();
        addAction(sources, label("clipboard"), busy, this::importClipboard);
        addAction(sources, label("qr"), busy, this::qrSources);
        panel.addView(sources, new LinearLayout.LayoutParams(-1, -2));
        addBlock(panel);
    }

    private void primary(boolean permission) {
        if (!permission) {
            send("primary", null);
            return;
        }
        Intent consent = VpnService.prepare(this);
        if (consent == null) ClientVpnService.request(this);
        else startActivityForResult(consent, VPN_CONSENT);
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == VPN_CONSENT && result == RESULT_OK) ClientVpnService.request(this);
        if (request == QR_FILE && result == RESULT_OK && data != null && data.getData() != null) {
            readQrFile(data.getData());
        }
    }

    @Override public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(request, permissions, results);
        if (request != QR_CAMERA_PERMISSION || isFinishing()) return;
        if (results.length > 0 && results[0] == PackageManager.PERMISSION_GRANTED) openQrCamera();
        else importError(label("qr_camera_unavailable"));
    }

    private void qrSources() {
        String[] sources = {label("qr_file"), label("qr_screenshot"), label("qr_camera")};
        new AlertDialog.Builder(this)
            .setTitle(label("qr"))
            .setItems(sources, (dialog, which) -> {
                if (which == 0) pickQrFile();
                if (which == 1) explainScreenshot();
                if (which == 2) requestQrCamera();
            })
            .setNegativeButton(label("cancel"), null)
            .show();
    }

    private void explainScreenshot() {
        new AlertDialog.Builder(this)
            .setTitle(label("qr_screenshot"))
            .setMessage(label("qr_screenshot_help"))
            .setPositiveButton(label("qr_file"), (dialog, which) -> pickQrFile())
            .setNegativeButton(label("cancel"), null)
            .show();
    }

    private void pickQrFile() {
        Intent picker = new Intent(Intent.ACTION_OPEN_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType("image/*");
        try {
            startActivityForResult(picker, QR_FILE);
        } catch (ActivityNotFoundException failure) {
            importError(label("qr_image_error"));
        }
    }

    private void requestQrCamera() {
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            openQrCamera();
        } else {
            requestPermissions(new String[] {Manifest.permission.CAMERA}, QR_CAMERA_PERMISSION);
        }
    }

    private void openQrCamera() {
        if (cameraScanner != null) cameraScanner.close();
        cameraScanner = new QrCameraScanner(this, label("qr_camera"), label("qr_camera_help"),
            label("cancel"), this::importDetected, () -> importError(label("qr_camera_unavailable")));
    }

    private void importClipboard() {
        ClipboardManager clipboard = getSystemService(ClipboardManager.class);
        ClipData clip = clipboard == null ? null : clipboard.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) {
            importError(label("clipboard_empty"));
            return;
        }
        ClipData.Item item = clip.getItemAt(0);
        CharSequence value = item.getText();
        Uri uri = item.getUri();
        if (value != null && !value.toString().trim().isEmpty()) {
            importDetected(value.toString());
        } else if (uri != null && clip.getDescription().hasMimeType("image/*")) {
            readQrFile(uri);
        } else if (uri != null && ("https".equalsIgnoreCase(uri.getScheme())
                || "http".equalsIgnoreCase(uri.getScheme()))) {
            importDetected(uri.toString());
        } else {
            importError(label("clipboard_empty"));
        }
    }

    private void importDetected(String value) {
        if (value.length() > MAX_TEXT_BYTES || value.getBytes(StandardCharsets.UTF_8).length > MAX_TEXT_BYTES) {
            importError(label("clipboard_large"));
            return;
        }
        try {
            submit(request("auto_import").put("text", value));
        } catch (JSONException failure) {
            importError(label("import_problem"));
        }
    }

    private void readQrFile(Uri uri) {
        new Thread(() -> {
            byte[] data;
            try (InputStream source = getContentResolver().openInputStream(uri)) {
                if (source == null) throw new IOException();
                data = readImageBytes(source);
            } catch (IOException | SecurityException failure) {
                runOnUiThread(() -> importError(label("qr_image_error")));
                return;
            }
            if (data == null) {
                runOnUiThread(() -> importError(label("qr_image_large")));
                return;
            }
            String result = NativeSession.decodeQrImage(data);
            runOnUiThread(() -> finishQrRead(result));
        }, "decode-qr").start();
    }

    private static byte[] readImageBytes(InputStream source) throws IOException {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        byte[] buffer = new byte[8192];
        for (int size; (size = source.read(buffer)) != -1; ) {
            if (size > MAX_IMAGE_BYTES - output.size()) return null;
            output.write(buffer, 0, size);
        }
        return output.toByteArray();
    }

    private void finishQrRead(String result) {
        if (isFinishing() || isDestroyed()) return;
        try {
            JSONObject payload = new JSONObject(result);
            if (payload.has("text")) importDetected(payload.getString("text"));
            else importError(payload.optString("error", label("qr_image_error")));
        } catch (JSONException failure) {
            importError(label("qr_image_error"));
        }
    }

    private void importError(String message) {
        if (isFinishing() || isDestroyed()) return;
        new AlertDialog.Builder(this)
            .setTitle(label("import_problem"))
            .setMessage(message)
            .setPositiveButton(android.R.string.ok, null)
            .show();
    }

    private void section(String title, JSONArray data, boolean busy, boolean subscription) {
        TextView heading = caption(title, 18, R.color.foreground);
        heading.setTypeface(null, Typeface.BOLD);
        addBlock(heading);
        if (data == null || data.length() == 0) {
            screen.addView(caption(label(subscription ? "empty_subscriptions" : "empty_profiles"),
                14, R.color.muted));
            return;
        }
        for (int index = 0; index < data.length(); index++) {
            JSONObject entry = data.optJSONObject(index);
            if (entry != null) addBlock(subscription ? subscription(entry, busy) : profile(entry, busy));
        }
    }

    private View subscription(JSONObject data, boolean busy) {
        LinearLayout card = card();
        card.addView(caption(data.optString("title"), 16, R.color.foreground));
        if (!data.isNull("error")) card.addView(caption(data.optString("error"), 14, R.color.accent));
        Button refresh = button(label("refresh"), () -> send("refresh", data.optString("uuid")));
        refresh.setEnabled(!busy);
        card.addView(refresh);
        return card;
    }

    private View profile(JSONObject data, boolean busy) {
        LinearLayout card = card();
        card.addView(caption(data.optString("name"), 16, R.color.foreground));
        String protocol = data.optString("protocol");
        if (!protocol.isEmpty()) card.addView(caption(protocol, 13, R.color.muted));
        String unavailable = data.optString("unavailable_reason");
        if (!unavailable.isEmpty()) {
            card.addView(caption(unavailable, 13, R.color.muted));
            return card;
        }
        boolean selected = data.optBoolean("selected");
        Button choose = button(label(selected ? "selected" : "select"),
            () -> send("select", data.optString("uuid")));
        choose.setEnabled(!busy && !selected);
        card.addView(choose);
        return card;
    }

    private void form(boolean subscription) {
        LinearLayout content = column();
        content.setPadding(dp(20), 0, dp(20), 0);
        EditText title = field(label(subscription ? "name" : "configuration_name"), 128, true);
        EditText value = field(label(subscription ? "url" : "import_text"),
            subscription ? 8192 : 1024 * 1024, subscription);
        content.addView(title);
        content.addView(value);
        new AlertDialog.Builder(this)
            .setTitle(label(subscription ? "add_subscription" : "import"))
            .setView(content)
            .setNegativeButton(label("cancel"), null)
            .setPositiveButton(label("save"), (dialog, which) ->
                save(subscription, title.getText().toString(), value.getText().toString()))
            .show();
    }

    private void save(boolean subscription, String title, String value) {
        JSONObject request = request(subscription ? "subscribe" : "import");
        try {
            request.put("title", title);
            request.put(subscription ? "url" : "text", value);
            submit(request);
        } catch (JSONException failure) {
            showUnavailable();
        }
    }

    private void send(String type, String uuid) {
        JSONObject request = request(type);
        try {
            if (uuid != null) request.put("uuid", uuid);
            submit(request);
        } catch (JSONException failure) {
            showUnavailable();
        }
    }

    private static JSONObject request(String type) {
        try {
            return new JSONObject().put("type", type);
        } catch (JSONException impossible) {
            throw new IllegalStateException(impossible);
        }
    }

    private void submit(JSONObject request) {
        if (!NativeSession.dispatch(request.toString())) refresh();
    }

    private void renderFailure() {
        screen.removeAllViews();
        screen.addView(caption(getApplicationInfo().loadLabel(getPackageManager()).toString(), 26, R.color.foreground));
        screen.addView(caption("Could not open the local client session.", 15, R.color.muted));
    }

    private void showUnavailable() {
        Toast.makeText(this, label("unavailable"), Toast.LENGTH_SHORT).show();
    }

    private void addBlock(View view) {
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(-1, -2);
        params.topMargin = dp(12);
        screen.addView(view, params);
    }

    private void addAction(LinearLayout parent, String text, boolean busy, Runnable listener) {
        Button child = button(text, listener);
        child.setEnabled(!busy);
        parent.addView(child, new LinearLayout.LayoutParams(0, -2, 1));
    }

    private LinearLayout card() {
        LinearLayout card = column();
        card.setPadding(dp(12), dp(12), dp(12), dp(12));
        GradientDrawable background = new GradientDrawable();
        background.setColor(getColor(R.color.surface));
        background.setCornerRadius(dp(14));
        card.setBackground(background);
        return card;
    }

    private LinearLayout column() {
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        return layout;
    }

    private LinearLayout row() {
        LinearLayout layout = new LinearLayout(this);
        layout.setGravity(Gravity.CENTER_VERTICAL);
        return layout;
    }

    private TextView caption(String text, int size, int colorResource) {
        TextView view = new TextView(this);
        view.setText(text);
        view.setTextColor(getColor(colorResource));
        view.setTextSize(size);
        return view;
    }

    private Button button(String text, Runnable listener) {
        Button button = new Button(this);
        button.setAllCaps(false);
        button.setText(text);
        button.setOnClickListener(view -> listener.run());
        return button;
    }

    private EditText field(String hint, int maxLength, boolean singleLine) {
        EditText field = new EditText(this);
        field.setHint(hint);
        field.setSingleLine(singleLine);
        field.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO);
        field.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            | (singleLine ? 0 : InputType.TYPE_TEXT_FLAG_MULTI_LINE));
        field.setFilters(new InputFilter[] {new InputFilter.LengthFilter(maxLength)});
        return field;
    }

    private String label(String key) {
        return strings.optString(key);
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
