#!/usr/bin/env python3
"""Run CPU-only AMF IDR and dynamic peak-rate checks against production helpers."""
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


BITRATE_TEST = r"""
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <wchar.h>
#include "core/Surface.h"
#include "components/VideoEncoderVCE.h"
#include "components/VideoEncoderHEVC.h"
enum { AV_CODEC_ID_H264 = 1, AV_CODEC_ID_HEVC = 2 };
typedef struct { int count; const wchar_t *names[2]; int64_t values[2], target, peak; } Encoder;
typedef struct { int64_t av_bitrate; Encoder *encoder; } AmfContext;
typedef struct { int id; } Codec;
typedef struct { AmfContext *priv_data; Codec *codec; int64_t bit_rate, rc_max_rate; } AVCodecContext;
#define av_log(...) ((void)0)
#undef AMF_ASSIGN_PROPERTY_INT64
#define AMF_ASSIGN_PROPERTY_INT64(res, e, k, v) do { \
    assert((e)->count < 2); (e)->names[(e)->count] = (k); \
    (e)->values[(e)->count++] = (v); (res) = AMF_OK; assert((res) == AMF_OK); \
    if (!wcscmp((k), AMF_VIDEO_ENCODER_TARGET_BITRATE) || !wcscmp((k), AMF_VIDEO_ENCODER_HEVC_TARGET_BITRATE)) (e)->target = (v); else (e)->peak = (v); \
    assert((e)->target <= (e)->peak); \
} while (0)
#include "amf_bitrate.h"
int main(void)
{
    for (int id = AV_CODEC_ID_H264; id <= AV_CODEC_ID_HEVC; ++id) {
        Encoder encoder = {.target = 20000000, .peak = 20000000}; Codec codec = {id}; AmfContext context = {20000000, &encoder};
        AVCodecContext av = {&context, &codec, 1000000, 1000000};
        for (int update = 0; update != 2; ++update) {
            encoder.count = 0;
            assert(reconfig_encoder(&av) == 0 && encoder.count == 2);
            const wchar_t *target = id == AV_CODEC_ID_H264 ? AMF_VIDEO_ENCODER_TARGET_BITRATE : AMF_VIDEO_ENCODER_HEVC_TARGET_BITRATE;
            const wchar_t *peak = id == AV_CODEC_ID_H264 ? AMF_VIDEO_ENCODER_PEAK_BITRATE : AMF_VIDEO_ENCODER_HEVC_PEAK_BITRATE;
            int t = wcscmp(encoder.names[0], target) != 0;
            assert(!wcscmp(encoder.names[t], target) && !wcscmp(encoder.names[1 - t], peak));
            assert(encoder.values[t] == av.bit_rate && encoder.values[1 - t] == av.rc_max_rate);
            av.bit_rate = av.rc_max_rate = 1000000000;
        }
        encoder.count = 0;
        assert(reconfig_encoder(&av) == 0 && encoder.count == 0);
        av.bit_rate = 4000000; av.rc_max_rate = 0;
        assert(reconfig_encoder(&av) == 0 && encoder.count == 1);
    }
    puts("1 AMF dynamic target/peak ceiling regression passed for AVC and HEVC");
}
"""



INPUT_TEST = r"""
#include <assert.h>
#include <stdio.h>
#include "amf_input_queue.h"

int main(void) {
    AmfInputQueue queue;
    bool ready[2] = {true, true};
    const auto available = [&] { return queue.available([&](std::size_t slot) { return ready[slot]; }); };
    const auto first = available();
    assert(first && queue.publish(*first, 10));
    const auto second = available();
    assert(second && *second != *first && queue.publish(*second, 20));
    assert(!available()); // admission stops before consuming a third captured frame
    assert(queue.front()->pts == 10);
    assert(!queue.publish(*first, 30));
    assert(queue.front()->pts == 10); // retry cannot overwrite the prepared input
    ready[*first] = false;
    queue.consumed();
    assert(!available()); // the codec consumed it, but the GPU has not released it
    assert(queue.front()->pts == 20);
    ready[*first] = true;
    assert(available() == first && queue.publish(*first, 30));
    assert(queue.front()->pts == 20);
    queue.consumed();
    assert(queue.front()->pts == 30);
    queue.consumed(); // drain the final frame without another capture arriving
    assert(!queue.front() && queue.latest() == first);
    assert(available() == second); // preserve the latest snapshot until needed

    for (int64_t pts = 40; pts < 10040; ++pts) {
        const auto slot = available();
        assert(slot && queue.publish(*slot, pts));
        assert(queue.front()->pts == pts);
        assert(!queue.publish(*slot, pts + 1));
        queue.consumed();
        assert(!queue.front() && queue.latest() == slot);
    }
    puts("1 AMF bounded input/GPU readiness/FIFO retry/static-tail regression passed");
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
        (work / 'input.cpp').write_text(INPUT_TEST)
        (work / 'surface.c').write_text(SURFACE_TEST)
        rate_patch = (root / 'res/vcpkg/ffmpeg/patch/0002-libavcodec-amfenc-reconfig-when-bitrate-change.patch').read_text()
        rate_body = '+static int reconfig_encoder' + rate_patch.split('+static int reconfig_encoder', 1)[1].split('\n int ff_amf_receive_packet', 1)[0]
        (work / 'amf_bitrate.h').write_text('\n'.join(line[1:] for line in rate_body.split('\n') if line.startswith('+')))
        (work / 'bitrate.c').write_text(BITRATE_TEST)
        include = root / 'vendor/hwcodec/externals/AMF_v1.4.35/amf/public/include'
        latch = root / 'vendor/hwcodec/cpp/ffmpeg_vram'
        runtime = ['-fms-runtime-lib=static'] if os.name == 'nt' else []
        for source, compiler, standard in (
            ('latch.cpp', args.cxx or args.cc, 'c++17'),
            ('input.cpp', args.cxx or args.cc, 'c++17'),
            ('surface.c', args.cc, 'c11'),
            ('bitrate.c', args.cc, 'c11'),
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
