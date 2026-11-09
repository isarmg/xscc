"""Offline, read-only verification; never executes driver code or native APIs."""
from pathlib import Path
import difflib,hashlib,json
ROOT=Path(__file__).resolve().parent
REV='d437ebc9b44a14ce6e5cc9c8b7f6beb08d6faf77'
DRIVER='Virtual Display Driver (HDR)/MttVDD/Driver.cpp'
HELPER='Virtual Display Driver (HDR)/MttVDD/SwapChainStop.h'
INF='Virtual Display Driver (HDR)/MttVDD/MttVDD.inf'
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
        if name not in {DRIVER,INF}: assert physical(ROOT/'source'/name).read_bytes()==data
    assert {str(p.relative_to(ROOT/'upstream')).replace('\\','/') for p in (ROOT/'upstream').rglob('*') if p.is_file()}==expected
    assert {str(p.relative_to(ROOT/'source')).replace('\\','/') for p in (ROOT/'source').rglob('*') if p.is_file()}==expected|{HELPER}
    original=(ROOT/'upstream'/DRIVER).read_text(encoding='utf-8')
    changed=(ROOT/'source'/DRIVER).read_text(encoding='utf-8')
    delta=''.join(difflib.unified_diff(original.splitlines(keepends=True),changed.splitlines(keepends=True),fromfile='a/'+DRIVER,tofile='b/'+DRIVER))
    assert delta==(ROOT/'patches/0001-prioritize-swapchain-stop.patch').read_text()
    original_inf=(ROOT/'upstream'/INF).read_bytes().decode('utf-16').replace('\r\n','\n')
    changed_inf=(ROOT/'source'/INF).read_bytes().decode('utf-16').replace('\r\n','\n')
    inf_delta=''.join(difflib.unified_diff(original_inf.splitlines(keepends=True),changed_inf.splitlines(keepends=True),fromfile='a/'+INF,tofile='b/'+INF))
    assert inf_delta==(ROOT/'patches/0002-use-isolated-windows11-umdf-inf.patch').read_text()
    assert changed_inf.count('Include=WUDFRD.inf')==3
    assert all(('Needs='+section) in changed_inf for section in ['WUDFRD.NT','WUDFRD.NT.HW','WUDFRD.NT.Services'])
    assert 'ServiceBinary=%13%\\MttVDD.dll' in changed_inf and 'UMDriverCopy=13' in changed_inf
    assert '[Standard.NT$ARCH$.10.0...22000]' in changed_inf
    assert 'Root\\MttVDD' in changed_inf and 'UmdfExtensions = IddCx0102' in changed_inf
    assert 'WUDFRd.sys' not in changed_inf and '[WUDFRD_ServiceInstall]' not in changed_inf

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
    print(json.dumps({'verified_files':len(expected),'revision':REV,'patches':['stop-priority-only','isolated-windows11-umdf-inf'],'native_driver_executed':False}))
if __name__=='__main__': main()
