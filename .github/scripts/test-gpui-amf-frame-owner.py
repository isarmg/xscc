#!/usr/bin/env python3
"""Compile and run CPU-only AMF frame ownership checks; never loads the AMF runtime."""
import argparse
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import tempfile

TEST = r"""
#include <assert.h>
#include <stdio.h>
#ifdef _WIN32
#include <windows.h>
#include <unknwn.h>
#define THREAD_RESULT DWORD WINAPI
#else
#include <pthread.h>
#define THREAD_RESULT void *
#endif
#include "AMF/core/Variant.h"
#include "amfenc_frame_ref.h"

static void free_buffer(void *opaque, uint8_t *data)
{
    ++*(int *)opaque;
    av_free(data);
}

static AVFrame *new_frame(int *released)
{
    AVFrame *frame = av_frame_alloc();
    uint8_t *data = av_malloc(1);
    assert(frame && data);
    frame->buf[0] = av_buffer_create(data, 1, free_buffer, released, 0);
    assert(frame->buf[0]);
    return frame;
}

static void assign_owner(AMFVariantStruct *value, const AVFrame *frame)
{
    AMFInterface *owner = amf_frame_owner_create(frame);
    assert(owner);
    assert(AMFVariantAssignInterface(value, owner) == AMF_OK);
    owner->pVtbl->Release(owner);
}

static void discarded_properties(void)
{
    // No output, failed submission, and discarded tail use the same property contract.
    for (int copies = 0; copies != 3; ++copies) {
        int released = 0;
        AVFrame *frame = new_frame(&released);
        AMFVariantStruct local = {0}, input = {0}, output = {0};
        assign_owner(&local, frame);
        av_frame_free(&frame);
        if (copies)
            assert(AMFVariantCopy(&input, &local) == AMF_OK);
        if (copies == 2)
            assert(AMFVariantCopy(&output, &input) == AMF_OK);
        AMFVariantClear(&local);
        assert(released == (copies == 0));
        AMFVariantClear(&input);
        assert(released == (copies != 2));
        AMFVariantClear(&output);
        assert(released == 1);
    }
}

static void completed_output_returns_pool_frame(void)
{
    int released = 0;
    AVFrame *frame = new_frame(&released);
    AMFVariantStruct input = {0}, output = {0}, retrieved = {0};
    AMFInterface *owner = NULL;
    AMFGuid base_iid = IID_AMFInterface(), invalid_iid = {0};
    assign_owner(&input, frame);
    assert(!av_frame_is_writable(frame));
    assert(AMFVariantCopy(&output, &input) == AMF_OK);
    assert(AMFVariantCopy(&retrieved, &output) == AMF_OK);
    AMFInterface *iface = AMFVariantInterface(&retrieved);
    assert(iface->pVtbl->QueryInterface(iface, &invalid_iid, (void **)&owner) == AMF_NO_INTERFACE);
    assert(!owner);
    assert(iface->pVtbl->QueryInterface(iface, &base_iid, (void **)&owner) == AMF_OK);
    owner->pVtbl->Release(owner);
    assert(iface->pVtbl->QueryInterface(iface, &amf_frame_owner_iid, (void **)&owner) == AMF_OK);
    amf_frame_owner_release_frame(owner);
    assert(av_frame_is_writable(frame)); // AMF still retains both properties.
    owner->pVtbl->Release(owner);
    AMFVariantClear(&retrieved);
    AMFVariantClear(&input);
    AMFVariantClear(&output);
    assert(released == 0);
    av_frame_free(&frame);
    assert(released == 1);
}

static THREAD_RESULT concurrent_references(void *opaque)
{
    AMFInterface *owner = opaque;
    for (int i = 0; i != 20000; ++i) {
        AMFVariantStruct value = {0};
        owner->pVtbl->Acquire(owner);
        assert(AMFVariantAssignInterface(&value, owner) == AMF_OK);
        AMFVariantClear(&value);
        owner->pVtbl->Release(owner);
    }
    return 0;
}

static THREAD_RESULT release_last_reference(void *opaque)
{
    AMFInterface *owner = opaque;
    owner->pVtbl->Release(owner);
    return 0;
}

static void concurrent_property_references(void)
{
    int released = 0;
    AVFrame *frame = new_frame(&released);
    AMFVariantStruct property = {0};
    assign_owner(&property, frame);
    av_frame_free(&frame);
    AMFInterface *owner = AMFVariantInterface(&property);
#ifdef _WIN32
    HANDLE threads[4];
    for (int i = 0; i != 4; ++i) {
        threads[i] = CreateThread(NULL, 0, concurrent_references, owner, 0, NULL);
        assert(threads[i]);
    }
    for (int i = 0; i != 4; ++i) {
        assert(WaitForSingleObject(threads[i], 10000) == WAIT_OBJECT_0);
        CloseHandle(threads[i]);
    }
#else
    pthread_t threads[4];
    for (int i = 0; i != 4; ++i)
        assert(!pthread_create(&threads[i], NULL, concurrent_references, owner));
    for (int i = 0; i != 4; ++i)
        assert(!pthread_join(threads[i], NULL));
#endif
    assert(released == 0);
    AMFVariantClear(&property);
    assert(released == 1);

    released = 0;
    frame = new_frame(&released);
    owner = amf_frame_owner_create(frame);
    assert(owner);
    av_frame_free(&frame);
#ifdef _WIN32
    HANDLE final_thread = CreateThread(NULL, 0, release_last_reference, owner, 0, NULL);
    assert(final_thread);
    assert(WaitForSingleObject(final_thread, 10000) == WAIT_OBJECT_0);
    CloseHandle(final_thread);
#else
    pthread_t final_thread;
    assert(!pthread_create(&final_thread, NULL, release_last_reference, owner));
    assert(!pthread_join(final_thread, NULL));
#endif
    assert(released == 1);
}

int main(void)
{
    discarded_properties();
    completed_output_returns_pool_frame();
    concurrent_property_references();
    puts("3 AMF owner CPU lifecycle tests passed");
    return 0;
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cc', default='cc')
    parser.add_argument('--pkg-config', default='pkg-config')
    parser.add_argument('--cflags')
    parser.add_argument('--ldflags')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    patch = (root / 'res/vcpkg/ffmpeg/patch/0013-amf-own-pending-hardware-frames.patch').read_text()
    added = patch.split('+++ b/libavcodec/amfenc_frame_ref.h\n', 1)[1].splitlines()
    match = re.fullmatch(r'@@ -0,0 \+1,(\d+) @@', added[0])
    if not match or any(not line.startswith('+') for line in added[1:]):
        raise ValueError('Expected one complete added production owner header')
    header = '\n'.join(line[1:] for line in added[1:]) + '\n'
    if len(added) - 1 != int(match[1]):
        raise ValueError('Owner header patch line count does not match')
    cflags = args.cflags if args.cflags is not None else subprocess.check_output(
        [args.pkg_config, '--cflags', 'libavutil'], text=True)
    ldflags = args.ldflags if args.ldflags is not None else subprocess.check_output(
        [args.pkg_config, '--static', '--libs', 'libavutil'], text=True)
    library_flags = shlex.split(ldflags)
    if os.name == 'nt' and args.ldflags is None:
        # pkgconf emits MSVC search flags; clang's driver accepts -L instead.
        library_flags = ['-L' + flag[9:] if flag.lower().startswith(('-libpath:', '/libpath:'))
                         else flag for flag in library_flags]
    temporary_root = Path(tempfile.gettempdir()).resolve()
    with tempfile.TemporaryDirectory(prefix='xscc-amf-owner-', dir=temporary_root) as directory:
        work = Path(directory).resolve()
        if work.parent != temporary_root:
            raise ValueError('Ownership test directory escaped the intended temporary directory')
        (work / 'amfenc_frame_ref.h').write_text(header)
        (work / 'test.c').write_text(TEST)
        shutil.copytree(root / 'vendor/hwcodec/externals/AMF_v1.4.35/amf/public/include',
                        work / 'AMF')
        executable = work / ('test.exe' if os.name == 'nt' else 'test')
        runtime = ['-fms-runtime-lib=static'] if os.name == 'nt' else []
        command = [args.cc, '-std=c11', '-Wall', '-Wextra', '-Werror', *runtime,
                   '-I' + str(work), *shlex.split(cflags), str(work / 'test.c'),
                   *library_flags, '-o', str(executable)]
        subprocess.run(command, check=True, timeout=120)
        subprocess.run([str(executable)], check=True, timeout=20)


if __name__ == '__main__':
    main()
