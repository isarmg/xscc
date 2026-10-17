#!/usr/bin/env python3
"""Exercise the real vendored single-line key branch in a tiny JVM harness.

This extracts production source up to the remaining editing implementation; it
is not a test of Android's Editable/IME integration, which still needs devices.
"""
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / 'packaging/gpui-android/vendor/game-text-input/src/main/java/com/google/androidgamesdk/gametextinput/InputConnection.java'
source = SOURCE.read_text()
start = source.index('    private boolean processKeyEvent(KeyEvent event) {')
end = source.index('        // If no selection is set', start)
branch = source[start:end] + '        return false;\n    }\n'
harness = '''
public class EditorActionTest {
    static class KeyEvent {
        static final int ACTION_DOWN=0, ACTION_UP=1, KEYCODE_ENTER=66, KEYCODE_NUMPAD_ENTER=160;
        final int action, repeat, key; final boolean modifiers;
        KeyEvent(int a,int r,int k,boolean m) {action=a;repeat=r;key=k;modifiers=m;}
        int getKeyCode(){return key;} int getAction(){return action;}
        int getRepeatCount(){return repeat;} boolean hasNoModifiers(){return !modifiers;}
    }
    static class EditorInfo {static final int TYPE_TEXT_FLAG_MULTI_LINE=131072; int inputType,actionId=6;}
    static class Settings {EditorInfo mEditorInfo=new EditorInfo();}
    Settings settings=new Settings(); int actions;
    boolean sendEditorAction(int action) {if(action!=6)throw new AssertionError();actions++;return true;}
''' + branch + '''
    public static void main(String[] args) {
        EditorActionTest t=new EditorActionTest();
        for(int key:new int[]{66,160}) {
            int before=t.actions;
            if(!t.processKeyEvent(new KeyEvent(0,0,key,false)))throw new AssertionError();
            if(!t.processKeyEvent(new KeyEvent(0,1,key,false)))throw new AssertionError();
            if(!t.processKeyEvent(new KeyEvent(1,0,key,false)))throw new AssertionError();
            if(t.actions!=before+1)throw new AssertionError("duplicate action");
        }
        int before=t.actions;
        t.processKeyEvent(new KeyEvent(0,0,66,true));
        t.settings.mEditorInfo.inputType=EditorInfo.TYPE_TEXT_FLAG_MULTI_LINE;
        t.processKeyEvent(new KeyEvent(0,0,66,false));
        t.processKeyEvent(new KeyEvent(1,0,66,false));
        if(t.actions!=before)throw new AssertionError("changed multiline/modifier behavior");
        System.out.println("Vendored editor-action down/up/repeat regression passed");
    }
}
'''
java = shutil.which('java')
if not java:
    raise SystemExit('Requires an installed JDK; nothing is downloaded.')
with tempfile.TemporaryDirectory(prefix='gpui-editor-action-') as directory:
    source_path = Path(directory) / 'EditorActionTest.java'
    source_path.write_text(harness)
    subprocess.run([java, 'com.sun.tools.javac.Main', '-source', '17', '-target', '17',
                    '-Xlint:-options', str(source_path)], check=True)
    subprocess.run([java, '-cp', directory, 'EditorActionTest'], check=True)
