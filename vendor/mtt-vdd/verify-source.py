"""Offline, read-only verification; never executes driver code or native APIs."""
from pathlib import Path
import difflib,hashlib,json
ROOT=Path(__file__).resolve().parent
REV='d437ebc9b44a14ce6e5cc9c8b7f6beb08d6faf77'
DRIVER='Virtual Display Driver (HDR)/MttVDD/Driver.cpp'
HELPER='Virtual Display Driver (HDR)/MttVDD/SwapChainStop.h'
def digest(data): return hashlib.sha256(data).hexdigest()
def physical(path):
    for part in [path,*path.parents]:
        if part==ROOT.parent: break
        assert not part.is_symlink() and not getattr(part,'is_junction',lambda:False)(), 'Source traverses a reparse point'
    return path
def main():
    meta=json.loads((ROOT/'upstream.json').read_text())
    assert meta['revision']==REV and meta['tag']=='25.7.23' and meta['source_file_count']==16
    expected=set()
    for item in meta['files']:
        name=item['path'];expected.add(name)
        assert not Path(name).is_absolute() and '..' not in Path(name).parts
        data=physical(ROOT/'upstream'/name).read_bytes()
        assert len(data)==item['bytes'] and digest(data)==item['sha256']
        assert hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()==item['git_blob']
        if name!=DRIVER: assert physical(ROOT/'source'/name).read_bytes()==data
    assert {str(p.relative_to(ROOT/'upstream')).replace('\\','/') for p in (ROOT/'upstream').rglob('*') if p.is_file()}==expected
    assert {str(p.relative_to(ROOT/'source')).replace('\\','/') for p in (ROOT/'source').rglob('*') if p.is_file()}==expected|{HELPER}
    original=(ROOT/'upstream'/DRIVER).read_text(encoding='utf-8')
    changed=(ROOT/'source'/DRIVER).read_text(encoding='utf-8')
    delta=''.join(difflib.unified_diff(original.splitlines(keepends=True),changed.splitlines(keepends=True),fromfile='a/'+DRIVER,tofile='b/'+DRIVER))
    assert delta==(ROOT/'patches/0001-prioritize-swapchain-stop.patch').read_text()
    assert changed.count('xscc_mtt::before_acquire(WaitForSingleObject(m_hTerminateEvent.Get(), 0))')==1
    assert changed.count('xscc_mtt::after_pending_wait(WaitResult)')==1
    assert 'm_hTerminateEvent.Get(),\n\t\t\t\tm_hAvailableBufferEvent' in changed
    run=changed[changed.index('void SwapChainProcessor::RunCore()'):]
    assert run.index('xscc_mtt::before_acquire(')<run.index('IddCxSwapChainReleaseAndAcquireBuffer')
    # Preserve sole worker ownership: no timed-out join, detach, or thread termination is introduced.
    destructor=lambda s:s[s.index('SwapChainProcessor::~SwapChainProcessor()'):s.index('DWORD CALLBACK SwapChainProcessor::RunThread')]
    assert destructor(original)==destructor(changed)
    for item in meta['maintained_files']:
        assert digest((ROOT/item['path']).read_bytes())==item['sha256']
    assert (ROOT/'LICENSE').read_bytes()==(ROOT/'upstream/LICENSE').read_bytes()
    print(json.dumps({'verified_files':len(expected),'revision':REV,'patch':'stop-priority-only','native_driver_executed':False}))
if __name__=='__main__': main()
