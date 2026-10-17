#!/usr/bin/env python3
"""Compile and exercise AndroidImeBridge with minimal JVM Android/GameActivity stubs.

This tests the session gate, not Android IME rendering or real device behavior.
No SDK, Gradle download, or production test hooks are needed.
"""
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
BRIDGE = ROOT / 'packaging/gpui-android/app/src/main/java/org/xscc/gpui/AndroidImeBridge.java'
METHODS = '''
boolean beginBatchEdit()
boolean clearMetaKeyStates(int states)
void closeConnection()
boolean commitCompletion(CompletionInfo text)
boolean commitContent(InputContentInfo content, int flags, Bundle opts)
boolean commitCorrection(CorrectionInfo correction)
boolean commitText(CharSequence text, int cursor)
boolean commitText(CharSequence text, int cursor, TextAttribute attribute)
boolean deleteSurroundingText(int before, int after)
boolean deleteSurroundingTextInCodePoints(int before, int after)
boolean endBatchEdit()
boolean finishComposingText()
int getCursorCapsMode(int modes)
ExtractedText getExtractedText(ExtractedTextRequest request, int flags)
Handler getHandler()
CharSequence getSelectedText(int flags)
SurroundingText getSurroundingText(int before, int after, int flags)
CharSequence getTextAfterCursor(int length, int flags)
CharSequence getTextBeforeCursor(int length, int flags)
boolean performContextMenuAction(int id)
boolean performEditorAction(int action)
void performHandwritingGesture(HandwritingGesture gesture, Executor executor, IntConsumer consumer)
boolean performPrivateCommand(String action, Bundle data)
boolean performSpellCheck()
boolean previewHandwritingGesture(PreviewableHandwritingGesture gesture, CancellationSignal signal)
boolean replaceText(int start, int end, CharSequence text, int cursor, TextAttribute attribute)
boolean reportFullscreenMode(boolean enabled)
boolean requestCursorUpdates(int mode)
boolean requestCursorUpdates(int mode, int filter)
void requestTextBoundsInfo(RectF bounds, Executor executor, Consumer<TextBoundsInfoResult> consumer)
boolean sendKeyEvent(KeyEvent event)
boolean setComposingRegion(int start, int end)
boolean setComposingRegion(int start, int end, TextAttribute attribute)
boolean setComposingText(CharSequence text, int cursor)
boolean setComposingText(CharSequence text, int cursor, TextAttribute attribute)
boolean setImeConsumesInput(boolean consumes)
boolean setSelection(int start, int end)
TextSnapshot takeSnapshot()
'''.strip().splitlines()

STUBS = {
'android.content.Context': 'public class Context {}',
'android.os.Looper': '''public class Looper {
    private static final Looper MAIN = new Looper();
    private static final Thread UI = Thread.currentThread();
    public static Looper getMainLooper() { return MAIN; }
    public static Looper myLooper() { return Thread.currentThread() == UI ? MAIN : null; }
}''',
'android.os.Handler': 'public class Handler { public Handler(Looper looper) {} }',
'android.text.Editable': 'public interface Editable extends CharSequence {}',
'android.view.KeyEvent': 'public class KeyEvent { public int getKeyCode() { return 1; } }',
'android.view.View': '''public class View {
    public interface OnKeyListener { boolean onKey(View view, int code, KeyEvent event); }
    private OnKeyListener listener;
    public View(android.content.Context context) {}
    public void setOnKeyListener(OnKeyListener listener) { this.listener = listener; }
    public boolean dispatchKeyEvent(KeyEvent event) { return listener != null && listener.onKey(this, event.getKeyCode(), event); }
    public android.view.inputmethod.InputConnection onCreateInputConnection(android.view.inputmethod.EditorInfo info) { return null; }
}''',
'android.view.SurfaceView': 'public class SurfaceView extends View { public SurfaceView(android.content.Context c) { super(c); } }',
'android.view.inputmethod.EditorInfo': '''public class EditorInfo {
    public int inputType, actionId, imeOptions, initialSelStart, initialSelEnd;
}''',
'android.view.inputmethod.SurroundingText': '''public class SurroundingText {
    private final CharSequence text;
    private final int start, end, offset;
    public SurroundingText(CharSequence text, int start, int end, int offset) {
        this.text = text; this.start = start; this.end = end; this.offset = offset;
    }
    public CharSequence getText() { return text; }
    public int getSelectionStart() { return start; }
    public int getSelectionEnd() { return end; }
    public int getOffset() { return offset; }
}''',
'android.view.inputmethod.TextBoundsInfoResult': '''public class TextBoundsInfoResult {
    public static final int CODE_UNSUPPORTED = 0;
    public TextBoundsInfoResult(int code) {}
}''',
'android.view.inputmethod.BaseInputConnection': '''public class BaseInputConnection implements InputConnection {
    public android.text.Editable getEditable() { return null; }
}''',
'com.google.androidgamesdk.gametextinput.Settings': '''public final class Settings {
    public final android.view.inputmethod.EditorInfo info;
    public Settings(android.view.inputmethod.EditorInfo info, boolean forward) { this.info = info; }
}''',
'com.google.androidgamesdk.gametextinput.InputConnection': '''
import android.content.Context;
import android.view.View;
import android.view.KeyEvent;
import android.view.inputmethod.*;
import com.google.androidgamesdk.GameActivity;
public class InputConnection extends BaseInputConnection implements View.OnKeyListener {
    public State state = new State("", 0, 0, -1, -1);
    public int edits;
    private boolean visible;
    private final View view;
    private EditorInfo info;
    private GameActivity listener;
    public InputConnection(Context context, View view, Settings settings) {
        this.view = view; this.info = settings.info; view.setOnKeyListener(this);
    }
    public final InputConnection setListener(GameActivity listener) { this.listener = listener; return this; }
    public final void setSoftKeyboardActive(boolean visible, int flags) { this.visible = visible; if (!visible) restartInput(); }
    public final boolean getSoftKeyboardActive() { return visible; }
    public final EditorInfo getEditorInfo() { return info; }
    public final void setEditorInfo(EditorInfo info) { this.info = info; }
    public final void setState(State state) { this.state = state; if (listener != null) listener.stateChanged(state, false); restartInput(); }
    public void restartInput() { view.onCreateInputConnection(new EditorInfo()); }
    public boolean commitText(CharSequence text, int cursor) { ++edits; state.text = text.toString(); return endBatchEdit(); }
    public boolean setComposingText(CharSequence text, int cursor) { return commitText(text, cursor); }
    public boolean setComposingRegion(int start, int end) { ++edits; state.composingRegionStart = start; state.composingRegionEnd = end; return true; }
    public boolean setSelection(int start, int end) { ++edits; state.selectionStart = start; state.selectionEnd = end; return true; }
    public boolean finishComposingText() { ++edits; state.composingRegionStart = state.composingRegionEnd = -1; return true; }
    public boolean endBatchEdit() { if (listener != null) listener.stateChanged(state, false); return true; }
    public boolean performEditorAction(int action) { ++edits; if (listener != null) listener.onEditorAction(action); return true; }
    public boolean onKey(View view, int code, KeyEvent event) { return commitText("hardware", 1); }
    public CharSequence getTextBeforeCursor(int length, int flags) { return state.text; }
    public CharSequence getTextAfterCursor(int length, int flags) { return state.text; }
    public CharSequence getSelectedText(int flags) { return state.text; }
    public SurroundingText getSurroundingText(int before, int after, int flags) { return new SurroundingText(state.text, 0, 0, 0); }
    public void closeConnection() { finishComposingText(); }
}''',
'com.google.androidgamesdk.GameActivity': '''
import android.view.*;
import android.view.inputmethod.EditorInfo;
import com.google.androidgamesdk.gametextinput.*;
public class GameActivity extends android.content.Context {
    public InputConnection original;
    public void stateChanged(State state, boolean dismissed) {}
    public void onEditorAction(int action) {}
    public void setTextInputState(State state) { original.setState(state); }
    public void setImeEditorInfo(EditorInfo info) { original.setEditorInfo(info); }
    protected InputEnabledSurfaceView createSurfaceView() { return new InputEnabledSurfaceView(this); }
    protected class InputEnabledSurfaceView extends SurfaceView {
        private final InputConnection connection;
        public InputEnabledSurfaceView(GameActivity activity) {
            super(activity);
            connection = new InputConnection(activity, this, new Settings(new EditorInfo(), false)).setListener(activity);
            original = connection;
        }
        public InputConnection onCreateInputConnection(EditorInfo info) { return connection.getSoftKeyboardActive() ? connection : null; }
    }
}''',
}
for name in ['android.graphics.RectF', 'android.os.Bundle', 'android.os.CancellationSignal']:
    STUBS[name] = 'public class ' + name.rsplit('.', 1)[1] + ' {}'
for name in ['CompletionInfo', 'CorrectionInfo', 'ExtractedText', 'ExtractedTextRequest',
             'HandwritingGesture', 'InputContentInfo', 'PreviewableHandwritingGesture',
             'TextAttribute', 'TextSnapshot']:
    STUBS['android.view.inputmethod.' + name] = 'public class ' + name + ' {}'
STUBS['android.view.inputmethod.InputConnection'] = '''
import android.os.*;
import android.graphics.RectF;
import android.view.KeyEvent;
import java.util.concurrent.Executor;
import java.util.function.*;
public interface InputConnection {
    int HANDWRITING_GESTURE_RESULT_UNSUPPORTED = 2;
''' + '\n'.join('    default ' + method + ' { ' +
        {'boolean': 'return false;', 'int': 'return 0;', 'void': ''}.get(method.split()[0], 'return null;')
        + ' }' for method in METHODS) + '\n}'
STUBS['com.google.androidgamesdk.gametextinput.State'] = '''public final class State {
    public String text;
    public int selectionStart, selectionEnd, composingRegionStart, composingRegionEnd;
    public State(String text, int ss, int se, int cs, int ce) {
        this.text = text; selectionStart = ss; selectionEnd = se; composingRegionStart = cs; composingRegionEnd = ce;
    }
}'''

TEST = r'''
package org.xscc.gpui;
import android.view.*;
import android.view.inputmethod.*;
import com.google.androidgamesdk.gametextinput.State;
import java.lang.reflect.*;
import java.util.concurrent.Executor;
import java.util.function.*;
public final class BridgeTest extends AndroidImeBridge {
    static native long lastGeneration();
    static native int callbacks();
    static native int lastAction();
    static void check(boolean ok, String message) { if (!ok) throw new AssertionError(message); }
    static Object argument(Class<?> type) throws Exception {
        if (type == int.class) return 1;
        if (type == boolean.class) return true;
        if (type == CharSequence.class || type == String.class) return "late";
        if (type == Executor.class) return (Executor) Runnable::run;
        if (type == Consumer.class) return (Consumer<Object>) value -> {};
        if (type == IntConsumer.class) return (IntConsumer) value -> {};
        return type.getConstructor().newInstance();
    }
    static void rejectEveryMethod(InputConnection stale) throws Exception {
        for (Method method : InputConnection.class.getDeclaredMethods()) {
            // Missing newer overloads must fail the audit, even if a stub default is harmless.
            stale.getClass().getDeclaredMethod(method.getName(), method.getParameterTypes());
            Class<?>[] types = method.getParameterTypes();
            Object[] args = new Object[types.length];
            for (int i = 0; i < types.length; i++) args[i] = argument(types[i]);
            Object result = method.invoke(stale, args);
            if (method.getReturnType() == boolean.class) check(Boolean.FALSE.equals(result), method.toString());
            else if (method.getReturnType() == int.class) check(Integer.valueOf(0).equals(result), method.toString());
            else check(result == null, method.toString());
        }
    }
    public static void main(String[] args) throws Exception {
        System.load(args[0]);
        BridgeTest bridge = new BridgeTest();
        View surface = bridge.createSurfaceView();
        check(surface.onCreateInputConnection(new EditorInfo()) == null, "inactive connection");
        bridge.configureIme(0, 1, 5, 0, "zero", 0, 0, -1, -1, true);
        check(surface.onCreateInputConnection(new EditorInfo()) == null, "zero generation");
        bridge.configureIme(11, 1, 5, 0x1000000, "field", 5, 5, -1, -1, true);
        check(bridge.original.getEditorInfo().imeOptions == 0x1000005, "action in imeOptions");
        check(callbacks() == 0, "programmatic configure echo");
        InputConnection old = surface.onCreateInputConnection(new EditorInfo());
        check(old.commitText("first", 1), "active commit");
        check(lastGeneration() == 11 && callbacks() == 1, "source generation");
        check(old.performEditorAction(5) && lastGeneration() == 11 && lastAction() == 5, "action source");
        bridge.configureIme(12, 129, 6, 0, "password", 8, 8, 0, 8, true);
        InputConnection current = surface.onCreateInputConnection(new EditorInfo());
        int count = callbacks(), edits = bridge.original.edits;
        rejectEveryMethod(old);
        check(callbacks() == count && bridge.original.edits == edits, "stale delegation");
        check(bridge.original.state.text.equals("password") && bridge.original.state.composingRegionEnd == 8, "stale close/replace modified field");
        check(current.getTextBeforeCursor(10, 0).toString().equals("password"), "current getter");
        bridge.stateChanged(new State("untagged", 0, 0, -1, -1), false);
        bridge.onEditorAction(9);
        check(callbacks() == count, "untagged callback accepted");
        check(surface.dispatchKeyEvent(new KeyEvent()) && lastGeneration() == 12, "hardware source");
        bridge.configureIme(13, 1, 6, 0, "updated", 7, 7, -1, -1, true);
        rejectEveryMethod(current);
        current = surface.onCreateInputConnection(new EditorInfo());
        check(current.commitText("new", 1) && lastGeneration() == 13, "same-field replacement");
        InputConnection beforeHide = current;
        bridge.hideIme(14);
        check(bridge.original.state.text.isEmpty(), "hide did not clear backing text");
        check(surface.onCreateInputConnection(new EditorInfo()) == null, "hide active connection");
        rejectEveryMethod(beforeHide);
        bridge.configureIme(13, 1, 5, 0, "late configure", 0, 0, -1, -1, true);
        check(bridge.original.state.text.isEmpty(), "older queued operation after hide");
        bridge.configureIme(15, 1, 6, 0, "restored", 8, 8, -1, -1, true);
        current = surface.onCreateInputConnection(new EditorInfo());
        bridge.hideIme(14);
        check(current.commitText("latest", 1) && lastGeneration() == 15, "stale hide removed new field");
        InputConnection mainOnly = current;
        java.util.concurrent.atomic.AtomicReference<Throwable> error = new java.util.concurrent.atomic.AtomicReference<>();
        Thread thread = new Thread(() -> {
            try { check(!mainOnly.commitText("thread", 1) && mainOnly.getTextBeforeCursor(10, 0) == null, "off-UI-thread operation accepted"); }
            catch (Throwable failure) { error.set(failure); }
        });
        thread.start(); thread.join();
        check(error.get() == null, "off-thread rejection");
        System.out.println("PASS: all 38 framework methods audited; stale password, close, configure/hide, source-tagged action/hardware, and UI-thread gates");
    }
}
'''
C = '''#include <stdint.h>
static int64_t generation;
static int count, action;
void Java_org_xscc_gpui_AndroidImeBridge_nativeTextChanged(void *env, void *cls, int64_t gen, void *text, int ss, int se, int cs, int ce) { generation = gen; ++count; }
void Java_org_xscc_gpui_AndroidImeBridge_nativeAction(void *env, void *cls, int64_t gen, int value) { generation = gen; action = value; ++count; }
int64_t Java_org_xscc_gpui_BridgeTest_lastGeneration(void *env, void *cls) { return generation; }
int Java_org_xscc_gpui_BridgeTest_callbacks(void *env, void *cls) { return count; }
int Java_org_xscc_gpui_BridgeTest_lastAction(void *env, void *cls) { return action; }
'''

def main():
    java = shutil.which('java')
    cc = shutil.which('cc')
    if not java or not cc:
        raise SystemExit('Requires an installed JDK and C compiler; nothing is downloaded.')
    with tempfile.TemporaryDirectory(prefix='gpui-ime-java-') as directory:
        root = Path(directory)
        for name, body in STUBS.items():
            path = root / (name.replace('.', '/') + '.java')
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('package ' + name.rsplit('.', 1)[0] + ';\n' + body)
        test = root / 'org/xscc/gpui/BridgeTest.java'
        test.parent.mkdir(parents=True, exist_ok=True)
        test.write_text(TEST)
        (root / 'callbacks.c').write_text(C)
        library = root / 'callbacks.so'
        subprocess.run([cc, '-shared', '-fPIC', '-o', str(library), str(root / 'callbacks.c')], check=True)
        subprocess.run([java, 'com.sun.tools.javac.Main', '-source', '17', '-target', '17', '-Xlint:-options', '-d', str(root / 'classes'),
                        *map(str, root.rglob('*.java')), str(BRIDGE)], check=True)
        subprocess.run([java, '-cp', str(root / 'classes'), 'org.xscc.gpui.BridgeTest', str(library)], check=True)

if __name__ == '__main__':
    main()
