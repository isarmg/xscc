package org.xscc.gpui;

import android.graphics.RectF;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.text.Editable;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.CompletionInfo;
import android.view.inputmethod.CorrectionInfo;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.ExtractedText;
import android.view.inputmethod.ExtractedTextRequest;
import android.view.inputmethod.HandwritingGesture;
import android.view.inputmethod.InputContentInfo;
import android.view.inputmethod.PreviewableHandwritingGesture;
import android.view.inputmethod.SurroundingText;
import android.view.inputmethod.TextAttribute;
import android.view.inputmethod.TextBoundsInfoResult;
import android.view.inputmethod.TextSnapshot;

import com.google.androidgamesdk.GameActivity;
import com.google.androidgamesdk.gametextinput.InputConnection;
import com.google.androidgamesdk.gametextinput.Settings;
import com.google.androidgamesdk.gametextinput.State;

import java.util.concurrent.Executor;
import java.util.function.Consumer;
import java.util.function.IntConsumer;
import java.util.function.Supplier;

/** The platform-specific, generation-bound text input entry point used by GPUI. */
public class AndroidImeBridge extends GameActivity {
    private ImeSurfaceView imeSurface;
    private long generation;
    private long lastGeneration;
    private long epoch;
    private long dispatchGeneration;
    private long dispatchEpoch;
    private boolean applyingState;

    public static native void nativeTextChanged(long generation, String text,
            int selectionStart, int selectionEnd, int composingStart, int composingEnd);
    public static native void nativeAction(long generation, int action);

    private static boolean onUiThread() {
        return Looper.myLooper() == Looper.getMainLooper();
    }

    private static void requireUiThread() {
        if (!onUiThread()) {
            throw new IllegalStateException("IME state must be changed on the UI thread");
        }
    }

    @Override
    protected InputEnabledSurfaceView createSurfaceView() {
        imeSurface = new ImeSurfaceView();
        return imeSurface;
    }

    public void configureIme(long nextGeneration, int inputType, int action, int options,
            String text, int selectionStart, int selectionEnd, int composingStart,
            int composingEnd, boolean show) {
        requireUiThread();
        if (nextGeneration == 0 || Long.compareUnsigned(nextGeneration, lastGeneration) <= 0
                || imeSurface == null || imeSurface.connection == null) {
            return;
        }
        lastGeneration = nextGeneration;
        generation = 0;
        ++epoch;
        applyingState = true;
        try {
            // Invalidate old connections before a hide/restart can call closeConnection.
            imeSurface.connection.setSoftKeyboardActive(false, 0);
            EditorInfo info = new EditorInfo();
            info.inputType = inputType;
            info.actionId = action;
            info.imeOptions = options | action;
            info.initialSelStart = selectionStart;
            info.initialSelEnd = selectionEnd;
            setImeEditorInfo(info);
            super.setTextInputState(new State(text, selectionStart, selectionEnd,
                    composingStart, composingEnd));
            generation = nextGeneration;
        } finally {
            applyingState = false;
        }
        if (show) {
            imeSurface.connection.setSoftKeyboardActive(true, 0);
            imeSurface.connection.restartInput();
            imeSurface.connection.setSoftKeyboardActive(true, 0);
        }
    }

    public void hideIme(long nextGeneration) {
        requireUiThread();
        if (nextGeneration == 0 || Long.compareUnsigned(nextGeneration, lastGeneration) <= 0) {
            return;
        }
        lastGeneration = nextGeneration;
        generation = 0;
        ++epoch;
        if (imeSurface == null || imeSurface.connection == null) {
            return;
        }
        applyingState = true;
        try {
            imeSurface.connection.setSoftKeyboardActive(false, 0);
            super.setTextInputState(new State("", 0, 0, -1, -1));
            setImeEditorInfo(new EditorInfo());
        } finally {
            applyingState = false;
        }
    }

    @Override
    public void stateChanged(State state, boolean dismissed) {
        // Only synchronous, source-tagged operations may cross into the native queue.
        if (onUiThread() && !applyingState && dispatchGeneration != 0
                && dispatchGeneration == generation && dispatchEpoch == epoch) {
            nativeTextChanged(dispatchGeneration, state.text, state.selectionStart,
                    state.selectionEnd, state.composingRegionStart, state.composingRegionEnd);
        }
    }

    @Override
    public void onEditorAction(int action) {
        if (onUiThread() && !applyingState && dispatchGeneration != 0
                && dispatchGeneration == generation && dispatchEpoch == epoch) {
            nativeAction(dispatchGeneration, action);
        }
    }

    private <T> T dispatch(long sourceGeneration, long sourceEpoch, Supplier<T> operation) {
        long previousGeneration = dispatchGeneration;
        long previousEpoch = dispatchEpoch;
        dispatchGeneration = sourceGeneration;
        dispatchEpoch = sourceEpoch;
        try {
            return operation.get();
        } finally {
            dispatchGeneration = previousGeneration;
            dispatchEpoch = previousEpoch;
        }
    }

    private final class ImeSurfaceView extends InputEnabledSurfaceView {
        private InputConnection connection;

        ImeSurfaceView() {
            super(AndroidImeBridge.this);
        }

        @Override
        public void setOnKeyListener(View.OnKeyListener listener) {
            // GameActivity constructs the native-owned connection in the superclass constructor.
            if (connection == null && listener instanceof InputConnection) {
                connection = (InputConnection) listener;
            }
            super.setOnKeyListener((view, code, event) -> {
                if (!onUiThread() || applyingState || generation == 0 || listener == null) {
                    return false;
                }
                return dispatch(generation, epoch, () -> listener.onKey(view, code, event));
            });
        }

        @Override
        public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
            if (!onUiThread() || applyingState || generation == 0) {
                return null;
            }
            InputConnection delegate = super.onCreateInputConnection(outAttrs);
            if (delegate == null) {
                return null;
            }
            // Every replacement also invalidates the previous connection within this field.
            ++epoch;
            return new SessionConnection(delegate, generation, epoch);
        }
    }

    // GameActivity's covariant return type requires its concrete InputConnection subclass.
    // Use a detached view so the guard's constructor cannot replace surface key/insets listeners.
    private final class SessionConnection extends InputConnection {
        private final InputConnection delegate;
        private final long sourceGeneration;
        private final long sourceEpoch;
        private boolean closed;

        SessionConnection(InputConnection delegate, long sourceGeneration, long sourceEpoch) {
            super(AndroidImeBridge.this, new View(AndroidImeBridge.this),
                    new Settings(new EditorInfo(), false));
            this.delegate = delegate;
            this.sourceGeneration = sourceGeneration;
            this.sourceEpoch = sourceEpoch;
        }

        private boolean active() {
            return onUiThread() && !closed && !applyingState && sourceGeneration != 0
                    && sourceGeneration == generation && sourceEpoch == epoch;
        }

        private <T> T call(T rejected, Supplier<T> operation) {
            return active() ? dispatch(sourceGeneration, sourceEpoch, operation) : rejected;
        }

        @Override
        public Editable getEditable() {
            // Never expose a mutable backing buffer that could outlive this session.
            return null;
        }

        @Override
        public boolean beginBatchEdit() {
            return call(false, () -> delegate.beginBatchEdit());
        }

        @Override
        public boolean endBatchEdit() {
            return call(false, () -> delegate.endBatchEdit());
        }

        @Override
        public boolean commitText(CharSequence text, int cursor) {
            return call(false, () -> delegate.commitText(text, cursor));
        }

        @Override
        public boolean commitText(CharSequence text, int cursor, TextAttribute attributes) {
            return commitText(text, cursor);
        }

        @Override
        public boolean setComposingText(CharSequence text, int cursor) {
            return call(false, () -> delegate.setComposingText(text, cursor));
        }

        @Override
        public boolean setComposingText(CharSequence text, int cursor, TextAttribute attributes) {
            return setComposingText(text, cursor);
        }

        @Override
        public boolean setComposingRegion(int start, int end) {
            return call(false, () -> delegate.setComposingRegion(start, end));
        }

        @Override
        public boolean setComposingRegion(int start, int end, TextAttribute attributes) {
            return setComposingRegion(start, end);
        }

        @Override
        public boolean finishComposingText() {
            return call(false, () -> delegate.finishComposingText());
        }

        @Override
        public boolean setSelection(int start, int end) {
            return call(false, () -> delegate.setSelection(start, end));
        }

        @Override
        public boolean deleteSurroundingText(int before, int after) {
            return call(false, () -> delegate.deleteSurroundingText(before, after));
        }

        @Override
        public boolean deleteSurroundingTextInCodePoints(int before, int after) {
            return call(false, () -> delegate.deleteSurroundingTextInCodePoints(before, after));
        }

        @Override
        public boolean replaceText(int start, int end, CharSequence text, int cursor,
                TextAttribute attributes) {
            return call(false, () -> {
                delegate.beginBatchEdit();
                try {
                    return delegate.finishComposingText() && delegate.setSelection(start, end)
                            && delegate.commitText(text, cursor);
                } finally {
                    delegate.endBatchEdit();
                }
            });
        }

        @Override
        public boolean sendKeyEvent(KeyEvent event) {
            // BaseInputConnection re-posts keys, losing the originating session.
            return event != null && call(false,
                    () -> delegate.onKey(imeSurface, event.getKeyCode(), event));
        }

        @Override
        public boolean onKey(View view, int keyCode, KeyEvent event) {
            return sendKeyEvent(event);
        }

        @Override
        public boolean performEditorAction(int action) {
            return call(false, () -> delegate.performEditorAction(action));
        }

        @Override
        public boolean commitCompletion(CompletionInfo text) {
            return call(false, () -> delegate.commitCompletion(text));
        }

        @Override
        public boolean commitCorrection(CorrectionInfo correction) {
            return call(false, () -> delegate.commitCorrection(correction));
        }

        @Override
        public boolean performContextMenuAction(int id) {
            return call(false, () -> delegate.performContextMenuAction(id));
        }

        @Override
        public boolean performPrivateCommand(String action, Bundle data) {
            return call(false, () -> delegate.performPrivateCommand(action, data));
        }

        @Override
        public boolean clearMetaKeyStates(int states) {
            return call(false, () -> delegate.clearMetaKeyStates(states));
        }

        @Override
        public boolean reportFullscreenMode(boolean enabled) {
            return call(false, () -> delegate.reportFullscreenMode(enabled));
        }

        @Override
        public boolean setImeConsumesInput(boolean consumesInput) {
            return call(false, () -> delegate.setImeConsumesInput(consumesInput));
        }

        @Override
        public CharSequence getTextBeforeCursor(int length, int flags) {
            return call(null, () -> snapshot(delegate.getTextBeforeCursor(length, flags)));
        }

        @Override
        public CharSequence getTextAfterCursor(int length, int flags) {
            return call(null, () -> snapshot(delegate.getTextAfterCursor(length, flags)));
        }

        @Override
        public CharSequence getSelectedText(int flags) {
            return call(null, () -> snapshot(delegate.getSelectedText(flags)));
        }

        @Override
        public int getCursorCapsMode(int modes) {
            return call(0, () -> delegate.getCursorCapsMode(modes));
        }

        @Override
        public SurroundingText getSurroundingText(int before, int after, int flags) {
            return call(null, () -> {
                SurroundingText text = delegate.getSurroundingText(before, after, flags);
                return text == null ? null : new SurroundingText(text.getText().toString(),
                        text.getSelectionStart(), text.getSelectionEnd(), text.getOffset());
            });
        }

        @Override
        public ExtractedText getExtractedText(ExtractedTextRequest request, int flags) {
            // GameTextInput has no extracted-text implementation or session-bound monitors.
            return null;
        }

        @Override
        public TextSnapshot takeSnapshot() {
            return null;
        }

        @Override
        public Handler getHandler() {
            return active() ? new Handler(Looper.getMainLooper()) : null;
        }

        @Override
        public void closeConnection() {
            // The native-owned delegate is shared by later sessions. Never close it here.
            closed = true;
        }

        @Override
        public boolean commitContent(InputContentInfo content, int flags, Bundle options) {
            return false;
        }

        @Override
        public boolean requestCursorUpdates(int mode) {
            return false;
        }

        @Override
        public boolean requestCursorUpdates(int mode, int filter) {
            return false;
        }

        @Override
        public boolean performSpellCheck() {
            return false;
        }

        @Override
        public boolean previewHandwritingGesture(PreviewableHandwritingGesture gesture,
                CancellationSignal cancellationSignal) {
            return false;
        }

        @Override
        public void performHandwritingGesture(HandwritingGesture gesture, Executor executor,
                IntConsumer consumer) {
            if (executor != null && consumer != null) {
                executor.execute(() -> consumer.accept(HANDWRITING_GESTURE_RESULT_UNSUPPORTED));
            }
        }

        @Override
        public void requestTextBoundsInfo(RectF bounds, Executor executor,
                Consumer<TextBoundsInfoResult> consumer) {
            if (executor != null && consumer != null) {
                executor.execute(() -> consumer.accept(
                        new TextBoundsInfoResult(TextBoundsInfoResult.CODE_UNSUPPORTED)));
            }
        }
    }

    private static String snapshot(CharSequence text) {
        return text == null ? null : text.toString();
    }
}
