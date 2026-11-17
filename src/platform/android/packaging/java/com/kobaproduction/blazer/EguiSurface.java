package com.kobaproduction.blazer;

import android.opengl.GLSurfaceView;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.text.InputType;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;

final class EguiSurface extends GLSurfaceView {
    private final EguiActivity activity;
    private final long handle;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final Object scheduleLock = new Object();
    private final Runnable repaint = this::drawScheduledFrame;
    private final AtomicBoolean imeWanted = new AtomicBoolean();
    private long scheduledAt = Long.MAX_VALUE;
    private volatile boolean drawing;
    private volatile boolean closed;
    private boolean recheckIme;

    EguiSurface(EguiActivity activity, long handle) {
        super(activity);
        this.activity = activity;
        this.handle = handle;
        setFocusableInTouchMode(true);
        setEGLContextClientVersion(2);
        setPreserveEGLContextOnPause(true);
        setRenderer(new EguiRenderer(this));
        setRenderMode(RENDERMODE_WHEN_DIRTY);
    }

    long handle() {
        return handle;
    }

    void resumeDrawing() {
        drawing = true;
        super.onResume();
        scheduleRender(0);
    }

    void pauseDrawing() {
        synchronized (scheduleLock) {
            drawing = false;
            scheduledAt = Long.MAX_VALUE;
            main.removeCallbacks(repaint);
        }
        super.onPause();
        imeWanted.set(false);
    }

    void dispose() {
        closed = true;
        EguiNative.detach(this);
        EguiNative.destroy(handle);
    }

    void scheduleRender(long delayMillis) {
        synchronized (scheduleLock) {
            if (!drawing || closed) return;
            long now = SystemClock.uptimeMillis();
            long deadline = now + Math.min(Math.max(0, delayMillis), Long.MAX_VALUE - now);
            if (deadline >= scheduledAt) return;
            main.removeCallbacks(repaint);
            scheduledAt = deadline;
            main.postAtTime(repaint, deadline);
        }
    }

    private void drawScheduledFrame() {
        synchronized (scheduleLock) {
            scheduledAt = Long.MAX_VALUE;
            if (!drawing || closed) return;
        }
        requestRender();
    }

    boolean postToActivity(Consumer<EguiActivity> action) {
        if (!drawing || closed) return false;
        return main.post(() -> {
            if (drawing && !closed && !activity.isFinishing()) action.accept(activity);
        });
    }

    void rendererFailed() {
        postToActivity(activity -> activity.showFailure("Blazer could not render the egui surface"));
    }

    void submitInput(Runnable action) {
        if (closed) return;
        queueEvent(() -> {
            if (!closed) action.run();
        });
        scheduleRender(0);
    }

    void publishOutput(int flags, String copied, String url) {
        boolean wantsKeyboard = (flags & 1) != 0;
        boolean requestPaste = (flags & 2) != 0;
        boolean updateIme = imeWanted.getAndSet(wantsKeyboard) != wantsKeyboard
            || wantsKeyboard && recheckIme;
        recheckIme = false;
        if (!updateIme && !requestPaste && copied.isEmpty() && url.isEmpty()) return;
        postToActivity(activity -> {
            if (updateIme && imeWanted.get() == wantsKeyboard) showKeyboard(wantsKeyboard);
            if (!copied.isEmpty()) activity.copyToClipboard(copied);
            if (requestPaste) activity.readClipboard();
            if (!url.isEmpty()) activity.openUrl(url);
        });
    }

    private void showKeyboard(boolean visible) {
        InputMethodManager input = activity.getSystemService(InputMethodManager.class);
        if (input == null) return;
        if (visible) {
            requestFocus();
            input.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT);
        } else {
            input.hideSoftInputFromWindow(getWindowToken(), 0);
        }
    }

    @Override public boolean onTouchEvent(MotionEvent event) {
        int action = switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> 0;
            case MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> 1;
            case MotionEvent.ACTION_MOVE -> 2;
            case MotionEvent.ACTION_CANCEL -> 3;
            default -> -1;
        };
        if (action < 0) return super.onTouchEvent(event);
        if (action == 0) requestFocus();
        boolean allPointers = action == 2 || action == 3;
        int count = allPointers ? event.getPointerCount() : 1;
        int[] pointers = new int[count];
        float[] positions = new float[count * 2];
        for (int i = 0; i < count; i++) {
            int index = allPointers ? i : event.getActionIndex();
            pointers[i] = event.getPointerId(index);
            positions[i * 2] = event.getX(index);
            positions[i * 2 + 1] = event.getY(index);
        }
        submitInput(() -> {
            for (int i = 0; i < count; i++) {
                EguiNative.touch(handle, action, pointers[i], positions[i * 2], positions[i * 2 + 1]);
            }
            recheckIme |= action == 1;
        });
        if (event.getActionMasked() == MotionEvent.ACTION_UP) performClick();
        return true;
    }

    @Override public boolean performClick() {
        super.performClick();
        return true;
    }

    @Override public boolean onKeyDown(int code, KeyEvent event) {
        return sendKey(event) || super.onKeyDown(code, event);
    }

    @Override public boolean onKeyUp(int code, KeyEvent event) {
        return sendKey(event) || super.onKeyUp(code, event);
    }

    private boolean sendKey(KeyEvent event) {
        int code = event.getKeyCode();
        if (code == KeyEvent.KEYCODE_BACK || code == KeyEvent.KEYCODE_VOLUME_UP
            || code == KeyEvent.KEYCODE_VOLUME_DOWN || code == KeyEvent.KEYCODE_VOLUME_MUTE) return false;
        boolean down = event.getAction() == KeyEvent.ACTION_DOWN;
        int modifiers = KeyEvent.normalizeMetaState(event.getMetaState());
        int unicode = event.getUnicodeChar(modifiers);
        boolean shortcut = event.isCtrlPressed() || event.isMetaPressed();
        submitInput(() -> {
            EguiNative.key(handle, code, down, modifiers);
            if (down && !shortcut && Character.isValidCodePoint(unicode)
                && !Character.isISOControl(unicode)) {
                EguiNative.text(handle, new String(Character.toChars(unicode)));
            }
        });
        return true;
    }

    @Override public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override public InputConnection onCreateInputConnection(EditorInfo editor) {
        editor.inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE
            | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
        editor.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING;
        editor.initialSelStart = editor.initialSelEnd = -1;
        return new EguiInputConnection();
    }

    private final class EguiInputConnection extends BaseInputConnection {
        private String composing = "";

        EguiInputConnection() {
            super(EguiSurface.this, true);
        }

        @Override public boolean setComposingText(CharSequence text, int cursor) {
            composing = text.toString();
            sendIme(1, composing);
            return true;
        }

        @Override public boolean commitText(CharSequence text, int cursor) {
            composing = "";
            String value = text.toString();
            if ("\n".equals(value) || "\r".equals(value)) {
                submitInput(() -> {
                    EguiNative.ime(handle, 3, "");
                    EguiNative.key(handle, KeyEvent.KEYCODE_ENTER, true, 0);
                    EguiNative.key(handle, KeyEvent.KEYCODE_ENTER, false, 0);
                });
            } else {
                sendIme(2, value);
            }
            return true;
        }

        @Override public boolean finishComposingText() {
            if (!composing.isEmpty()) commitText(composing, 1);
            return true;
        }

        @Override public void closeConnection() {
            composing = "";
            sendIme(3, "");
            super.closeConnection();
        }

        @Override public boolean deleteSurroundingText(int before, int after) {
            return deleteSurroundingTextInCodePoints(before, after);
        }

        @Override public boolean deleteSurroundingTextInCodePoints(int before, int after) {
            if (before < 0 || after < 0) return false;
            submitInput(() -> EguiNative.deleteSurrounding(handle, before, after));
            return true;
        }

        @Override public boolean sendKeyEvent(KeyEvent event) {
            return sendKey(event);
        }

        @Override public boolean performContextMenuAction(int id) {
            if (id != android.R.id.paste && id != android.R.id.pasteAsPlainText) return false;
            activity.readClipboard();
            return true;
        }

        private void sendIme(int phase, String text) {
            submitInput(() -> EguiNative.ime(handle, phase, text));
        }
    }
}
