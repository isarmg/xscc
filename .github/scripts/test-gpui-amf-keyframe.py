#!/usr/bin/env python3
"""Run three CPU-only AMF IDR checks against the production latch and surface helper."""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile

LATCH_TEST = r"""
#include <assert.h>
#include <stdio.h>
#include "amf_keyframe_request.h"

int main(void)
{
    AmfKeyframeRequest request;
    assert(!request.request(false)); // unsupported keeps Host's restart fallback
    assert(!request.pending());
    assert(request.request(true));
    assert(request.request(true)); // coalesced, not two future IDRs
    request.submitted(-11); // EAGAIN did not accept this input
    assert(request.pending());
    request.submitted(-5); // other unsuccessful sends cannot consume it either
    assert(request.pending());
    int idrs = 0;
    for (int i = 0; i != 3; ++i) {
        idrs += request.pending();
        request.submitted(0);
    }
    assert(idrs == 1 && !request.pending());
    assert(request.request(true)); // a later recovery can request another IDR
    request.submitted(0);
    assert(!request.pending());
    puts("1 AMF owner-latch/coalescing/retry/fallback test passed");
    return 0;
}
"""

SURFACE_TEST = r"""
#include <assert.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <wchar.h>
#ifdef _WIN32
#include <windows.h>
#include <unknwn.h>
#endif
#include "core/Surface.h"
#include "components/VideoEncoderVCE.h"
#include "components/VideoEncoderHEVC.h"
#include "amfenc_keyframe.h"

typedef struct TestSurface {
    AMFSurface surface;
    int calls;
    int fail_at;
    const wchar_t *names[3];
    AMFVariantStruct values[3];
} TestSurface;

static AMF_RESULT AMF_STD_CALL set_property(AMFSurface *surface, const wchar_t *name,
                                           AMFVariantStruct value)
{
    TestSurface *test = (TestSurface *)surface;
    assert(test->calls < 3);
    test->names[test->calls] = name;
    test->values[test->calls] = value;
    ++test->calls;
    return test->calls == test->fail_at ? AMF_FAIL : AMF_OK;
}
static const AMFSurfaceVtbl vtable = { .SetProperty = set_property };

static TestSurface new_surface(int fail_at)
{
    TestSurface test;
    memset(&test, 0, sizeof(test));
    test.surface.pVtbl = &vtable;
    test.fail_at = fail_at;
    return test;
}

static void true_idr_with_parameter_sets(void)
{
    TestSurface avc = new_surface(0), hevc = new_surface(0);
    assert(amf_surface_force_idr(&avc.surface, false, true) == AMF_OK);
    assert(avc.calls == 3);
    assert(!wcscmp(avc.names[0], AMF_VIDEO_ENCODER_FORCE_PICTURE_TYPE));
    assert(avc.values[0].type == AMF_VARIANT_INT64);
    assert(AMFVariantInt64(&avc.values[0]) == AMF_VIDEO_ENCODER_PICTURE_TYPE_IDR);
    assert(!wcscmp(avc.names[1], AMF_VIDEO_ENCODER_INSERT_SPS));
    assert(!wcscmp(avc.names[2], AMF_VIDEO_ENCODER_INSERT_PPS));
    assert(avc.values[1].type == AMF_VARIANT_BOOL && AMFVariantBool(&avc.values[1]));
    assert(avc.values[2].type == AMF_VARIANT_BOOL && AMFVariantBool(&avc.values[2]));
    assert(amf_surface_force_idr(&hevc.surface, true, true) == AMF_OK);
    assert(hevc.calls == 2);
    assert(!wcscmp(hevc.names[0], AMF_VIDEO_ENCODER_HEVC_FORCE_PICTURE_TYPE));
    assert(hevc.values[0].type == AMF_VARIANT_INT64);
    assert(AMFVariantInt64(&hevc.values[0]) == AMF_VIDEO_ENCODER_HEVC_PICTURE_TYPE_IDR);
    assert(!wcscmp(hevc.names[1], AMF_VIDEO_ENCODER_HEVC_INSERT_HEADER));
    assert(hevc.values[1].type == AMF_VARIANT_BOOL && AMFVariantBool(&hevc.values[1]));
    // AUTO is explicit on the next surface, including reused hardware textures.
    avc = new_surface(0);
    hevc = new_surface(0);
    assert(amf_surface_force_idr(&avc.surface, false, false) == AMF_OK);
    assert(avc.calls == 3 && AMFVariantInt64(&avc.values[0]) == AMF_VIDEO_ENCODER_PICTURE_TYPE_NONE);
    assert(!AMFVariantBool(&avc.values[1]) && !AMFVariantBool(&avc.values[2]));
    assert(amf_surface_force_idr(&hevc.surface, true, false) == AMF_OK);
    assert(hevc.calls == 2 && AMFVariantInt64(&hevc.values[0]) == AMF_VIDEO_ENCODER_HEVC_PICTURE_TYPE_NONE);
    assert(!AMFVariantBool(&hevc.values[1]));
}
static void property_errors_stop_submission(void)
{
    for (int hevc = 0; hevc != 2; ++hevc) {
        int properties = hevc ? 2 : 3;
        for (int fail_at = 1; fail_at <= properties; ++fail_at) {
            TestSurface test = new_surface(fail_at);
            assert(amf_surface_force_idr(&test.surface, hevc, true) == AMF_FAIL);
            assert(test.calls == fail_at);
        }
    }
}
int main(void)
{
    true_idr_with_parameter_sets();
    property_errors_stop_submission();
    puts("2 AMF surface IDR/parameter-set/error-propagation tests passed");
    return 0;
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cc', default='cc')
    parser.add_argument('--cxx')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    patch = (root / 'res/vcpkg/ffmpeg/patch/0014-amf-owner-thread-idr.patch').read_text()
    added = patch.split('+++ b/libavcodec/amfenc_keyframe.h\n', 1)[1].splitlines()
    match = re.fullmatch(r'@@ -0,0 \+1,(\d+) @@', added[0])
    if not match or any(not line.startswith('+') for line in added[1:]):
        raise ValueError('Expected the complete added production AMF IDR header')
    if len(added) - 1 != int(match[1]):
        raise ValueError('IDR header patch line count differs from production')
    header = '\n'.join(line[1:] for line in added[1:]) + '\n'
    temporary_root = Path(tempfile.gettempdir()).resolve()
    with tempfile.TemporaryDirectory(prefix='xscc-amf-keyframe-', dir=temporary_root) as directory:
        work = Path(directory).resolve()
        if work.parent != temporary_root:
            raise ValueError('IDR test directory escaped its temporary root')
        (work / 'amfenc_keyframe.h').write_text(header)
        (work / 'latch.cpp').write_text(LATCH_TEST)
        (work / 'surface.c').write_text(SURFACE_TEST)
        include = root / 'vendor/hwcodec/externals/AMF_v1.4.35/amf/public/include'
        latch = root / 'vendor/hwcodec/cpp/ffmpeg_vram'
        runtime = ['-fms-runtime-lib=static'] if os.name == 'nt' else []
        for source, compiler, standard in (
            ('latch.cpp', args.cxx or args.cc, 'c++17'),
            ('surface.c', args.cc, 'c11'),
        ):
            executable = work / (source + ('.exe' if os.name == 'nt' else '.out'))
            subprocess.run(
                [compiler, '-std=' + standard, '-Wall', '-Wextra', '-Werror', *runtime,
                 '-I' + str(work), '-I' + str(include), '-I' + str(latch),
                 str(work / source), '-o', str(executable)],
                check=True, timeout=120,
            )
            subprocess.run([str(executable)], check=True, timeout=20)


if __name__ == '__main__':
    main()
