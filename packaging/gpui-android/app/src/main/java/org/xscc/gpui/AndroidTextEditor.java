package org.xscc.gpui;

import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.fonts.Font;
import android.graphics.fonts.FontFamily;
import android.text.Editable;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;

import com.google.androidgamesdk.gametextinput.State;

import java.io.IOException;

/** A real native editor over only the focused GPUI field's content rectangle. */
final class AndroidTextEditor extends EditText {
    private final AndroidImeBridge owner;
    private long generation;
    private boolean batchEdit;
    private Typeface mapleTypeface;

    AndroidTextEditor(AndroidImeBridge owner) {
        super(owner);
        this.owner = owner;
        setBackgroundColor(Color.TRANSPARENT);
        setPadding(0, 0, 0, 0);
        setIncludeFontPadding(false);
        setSingleLine(true);
        setGravity(Gravity.LEFT | Gravity.CENTER_VERTICAL);
        setShowSoftInputOnFocus(false);
        setOnEditorActionListener((view, action, event) -> {
            owner.editorAction(generation, action);
            return true;
        });
    }

    void configure(long nextGeneration, int inputType, int action, int options, State state,
            float x, float y, float width, float height, float fontSize,
            int color, int weight, boolean italic, String family) {
        generation = 0;
        batchEdit = false;
        setSingleLine(true);
        setInputType(inputType);
        setImeOptions(options | action);
        updatePresentation(x, y, width, height, fontSize, color, weight, italic, family);
        setText(state.text);
        setSelection(state.selectionStart, state.selectionEnd);
        if (state.composingRegionStart >= 0 && state.composingRegionEnd >= 0) {
            new BaseInputConnection(this, true) {
                @Override public Editable getEditable() { return AndroidTextEditor.this.getText(); }
            }.setComposingRegion(state.composingRegionStart, state.composingRegionEnd);
        }
        generation = nextGeneration;
        setVisibility(View.VISIBLE);
        requestFocus();
    }

    void updatePresentation(float x, float y, float width, float height, float fontSize,
            int color, int weight, boolean italic, String family) {
        ViewGroup content = owner.findViewById(android.R.id.content);
        if (getParent() == null) {
            content.addView(this);
        }
        int[] surfacePosition = new int[2];
        int[] contentPosition = new int[2];
        owner.editorSurface().getLocationInWindow(surfacePosition);
        content.getLocationInWindow(contentPosition);
        FrameLayout.LayoutParams layout = new FrameLayout.LayoutParams(
                Math.max(1, Math.round(width)), Math.max(1, Math.round(height)), Gravity.TOP | Gravity.LEFT);
        layout.leftMargin = surfacePosition[0] - contentPosition[0] + Math.round(x);
        layout.topMargin = surfacePosition[1] - contentPosition[1] + Math.round(y);
        setLayoutParams(layout);
        setTypeface(editorTypeface(family, weight, italic));
        setTextSize(TypedValue.COMPLEX_UNIT_PX, fontSize);
        setTextColor(color);
    }

    private Typeface editorTypeface(String family, int weight, boolean italic) {
        if ("Maple Mono Normal NL".equals(family)) {
            if (mapleTypeface == null) {
                try {
                    Font regular = new Font.Builder(owner.getAssets(), "MapleMonoFull-Regular.ttf")
                            .setWeight(400).build();
                    Font bold = new Font.Builder(owner.getAssets(), "MapleMonoFull-Bold.ttf")
                            .setWeight(700).build();
                    FontFamily faces = new FontFamily.Builder(regular).addFont(bold).build();
                    mapleTypeface = new Typeface.CustomFallbackBuilder(faces)
                            .setSystemFallback("sans-serif").build();
                } catch (IOException error) {
                    throw new IllegalStateException("Bundled Maple fonts could not be loaded", error);
                }
            }
            return Typeface.create(mapleTypeface, weight, italic);
        }
        // Keep the diagnostic editor's default alias meaningful on Android.
        String systemFamily = ".SystemUIFont".equals(family) ? "sans-serif" : family;
        return Typeface.create(Typeface.create(systemFamily, Typeface.NORMAL), weight, italic);
    }

    void activate(boolean show, boolean menu) {
        owner.getSystemService(InputMethodManager.class).restartInput(this);
        showInteraction(show, menu);
    }

    void showInteraction(boolean show, boolean menu) {
        long nextGeneration = generation;
        InputMethodManager manager = owner.getSystemService(InputMethodManager.class);
        if (show) {
            manager.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT);
        }
        if (menu) {
            // The original pointer sequence belonged to the SurfaceView. Ask the
            // native editor to open its menu at the synchronized selection,
            // without inventing another touch sequence or using private APIs.
            post(() -> {
                if (generation == nextGeneration && owner.editorActive(nextGeneration)) {
                    performAccessibilityAction(AccessibilityNodeInfo.ACTION_LONG_CLICK, null);
                }
            });
        }
    }

    void hideEditor() {
        generation = 0;
        batchEdit = false;
        clearFocus();
        setVisibility(View.GONE);
        setText("");
    }

    @Override
    public android.view.inputmethod.InputConnection onCreateInputConnection(EditorInfo info) {
        if (owner == null || !owner.editorActive(generation)) { return null; }
        owner.closeEditorConnection();
        android.view.inputmethod.InputConnection delegate = super.onCreateInputConnection(info);
        if (delegate == null) { return null; }
        android.view.inputmethod.InputConnection protectedConnection = owner.protectEditorConnection(delegate, generation);
        publishState();
        return protectedConnection;
    }

    @Override
    protected void onTextChanged(CharSequence text, int start, int before, int count) {
        super.onTextChanged(text, start, before, count);
        changed();
    }

    @Override
    protected void onSelectionChanged(int start, int end) {
        super.onSelectionChanged(start, end);
        changed();
    }

    @Override
    public void onBeginBatchEdit() {
        super.onBeginBatchEdit();
        batchEdit = true;
    }

    @Override
    public void onEndBatchEdit() {
        super.onEndBatchEdit();
        batchEdit = false;
        changed();
    }

    private void changed() {
        if (owner != null && !owner.dispatchingEditorInput()) { publishState(); }
    }

    void publishState() {
        if (owner == null || batchEdit || !owner.editorActive(generation)) { return; }
        Editable text = getText();
        owner.editorChanged(generation, new State(text.toString(), getSelectionStart(), getSelectionEnd(),
                BaseInputConnection.getComposingSpanStart(text), BaseInputConnection.getComposingSpanEnd(text)));
    }

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        boolean handled = super.onTouchEvent(event);
        if (event.getActionMasked() == MotionEvent.ACTION_UP && owner.editorActive(generation)) {
            owner.getSystemService(InputMethodManager.class).showSoftInput(this, InputMethodManager.SHOW_IMPLICIT);
        }
        return handled;
    }
}
