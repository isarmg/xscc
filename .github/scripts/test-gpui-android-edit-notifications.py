#!/usr/bin/env python3
"""Exercise real vendored edit/batch methods without an Android SDK.

The small BaseInputConnection fixture models nested editing calls. It verifies
notification transactions, not an Android IME or the device's editable buffer.
"""
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / 'packaging/gpui-android/vendor/game-text-input/src/main/java/com/google/androidgamesdk/gametextinput/InputConnection.java'
source = SOURCE.read_text(encoding='utf-8')

def method(signature):
    start = source.index('    ' + signature)
    opening = source.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end] + '\n'

methods = '\n'.join(method(signature) for signature in [
    'public final void setState(State state)',
    'public boolean setSelection(int start, int end)',
    'public boolean setComposingText(CharSequence text, int newCursorPosition)',
    'public boolean setComposingRegion(int start, int end)',
    'public boolean finishComposingText()',
    'public boolean beginBatchEdit()',
    'public boolean endBatchEdit()',
    'private boolean edit(java.util.function.BooleanSupplier operation)',
    'public boolean commitText(CharSequence text, int newCursorPosition)',
    'public boolean deleteSurroundingText(int beforeLength, int afterLength)',
    'public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength)',
    'private final void stateUpdated()',
])
fields = source[source.index('    private int mBatchDepth;'):source.index('    /*', source.index('    private int mBatchDepth;'))]
harness = r'''
import java.util.ArrayList;
class Editable {
    String text=""; int start,end,cs=-1,ce=-1;
    void clear(){text="";} void clearSpans(){cs=ce=-1;}
    void insert(int at,CharSequence value){text=text.substring(0,at)+value+text.substring(at);}
    public String toString(){return text;}
}
class Pair {int first,second; Pair(int a,int b){first=a;second=b;}}
class State {
    String text; int selectionStart,selectionEnd,composingRegionStart,composingRegionEnd;
    State(String t,int s,int e,int cs,int ce){text=t;selectionStart=s;selectionEnd=e;composingRegionStart=cs;composingRegionEnd=ce;}
}
class EditorInfo {int initialSelStart,initialSelEnd;}
class Settings {EditorInfo mEditorInfo=new EditorInfo();}
interface Listener {void stateChanged(State state, boolean dismissed);}
class Base {
    Editable getEditable(){throw new AssertionError();}
    boolean setSelection(int s,int e){getEditable().start=s;getEditable().end=e;return true;}
    boolean setComposingRegion(int s,int e){getEditable().cs=s;getEditable().ce=e;return true;}
    boolean finishComposingText(){getEditable().cs=getEditable().ce=-1;return true;}
    boolean commitText(CharSequence value,int cursor){
        Editable e=getEditable();
        int at=Math.min(e.start,e.end),end=Math.max(e.start,e.end);
        e.text=e.text.substring(0,at)+value+e.text.substring(end);
        setSelection(at+value.length(),at+value.length());
        finishComposingText();
        return true;
    }
    boolean setComposingText(CharSequence value,int cursor){
        Editable e=getEditable(); int at=Math.min(e.start,e.end);
        commitText(value,cursor); setComposingRegion(at,at+value.length());return true;
    }
    boolean deleteSurroundingText(int before,int after){
        Editable e=getEditable();int start=Math.max(0,e.start-before),end=Math.min(e.text.length(),e.end+after);
        e.text=e.text.substring(0,start)+e.text.substring(end);setSelection(start,start);return true;
    }
    boolean deleteSurroundingTextInCodePoints(int before,int after){return deleteSurroundingText(before,after);}
}
public class EditNotifications extends Base {
    final Editable mEditable=new Editable(); final Settings settings=new Settings();
    final ArrayList<State> updates=new ArrayList<>();
    Listener listener=(state,dismissed)->updates.add(state);
    Editable getEditable(){return mEditable;}
    Pair getSelection(){return new Pair(mEditable.start,mEditable.end);}
    Pair getComposingRegion(){return new Pair(mEditable.cs,mEditable.ce);}
    void restartInput(){}
''' + fields + methods + r'''
    static void check(boolean ok,String reason){if(!ok)throw new AssertionError(reason);}
    public static void main(String[] args){
        EditNotifications t=new EditNotifications();
        // Password IMEs can send one commit, no composing/batch callbacks.
        t.commitText("p密😀",1);
        check(t.updates.size()==1,"standalone password commit was not one complete notification");
        State committed=t.updates.get(0);
        check(committed.text.equals("p密😀")&&committed.selectionEnd==4&&committed.composingRegionStart==-1,"partial edit snapshot");
        t.deleteSurroundingTextInCodePoints(2,0);
        check(t.updates.size()==2&&t.updates.get(1).text.equals("p密"),"standalone delete missing");
        // Nested IME batches must not leak intermediate composition/selection.
        t.updates.clear();t.beginBatchEdit();t.beginBatchEdit();
        t.setComposingText("码",1);t.finishComposingText();t.setSelection(0,0);
        check(t.updates.isEmpty(),"published a partial batch");
        check(t.endBatchEdit(),"nested batch ended early");
        check(t.updates.isEmpty(),"published nested batch early");
        check(!t.endBatchEdit(),"last batch remained open");
        check(t.updates.size()==1&&t.updates.get(0).selectionEnd==0,"final batch notification missing");
        // Switching address -> password abandons the old batch; native snapshot
        // must not echo to Rust, and the new session can commit immediately.
        t.beginBatchEdit();t.commitText("old",1);t.updates.clear();
        t.setState(new State("",0,0,-1,-1));
        check(t.updates.isEmpty(),"native snapshot echoed to Rust");
        t.commitText("new",1);
        check(t.updates.size()==1&&t.updates.get(0).text.equals("new"),"stale batch blocked new focus");
        t.endBatchEdit();check(t.updates.size()==1,"unmatched batch duplicated edit");
        System.out.println("Vendored IME standalone password/delete, nested batch and focus reset regressions passed");
    }
}
'''
java = shutil.which('java')
if not java:
    raise SystemExit('Requires an installed JDK; nothing is downloaded.')
with tempfile.TemporaryDirectory(prefix='gpui-edit-notifications-') as directory:
    path = Path(directory) / 'EditNotifications.java'
    path.write_text(harness, encoding='utf-8')
    subprocess.run([java, 'com.sun.tools.javac.Main', '-source', '17', '-target', '17', '-Xlint:-options', '-encoding', 'UTF-8', str(path)], check=True)
    subprocess.run([java, '-cp', directory, 'EditNotifications'], check=True)
