#!/usr/bin/env python3
"""Exercise the production FFmpeg/GPU retirement loop with CPU-only codec/context mocks."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile

TEST = r"""
#include <assert.h>
#include <algorithm>
#include <cerrno>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <stdio.h>
#include <thread>
#include <type_traits>

using HRESULT = int32_t;
using BOOL = int;
constexpr HRESULT S_OK = 0, S_FALSE = 1;
constexpr BOOL FALSE = 0, TRUE = 1;
#define FAILED(value) ((value) < 0)
#define AVERROR(value) (-(value))
constexpr int AVERROR_EOF = -10000;
inline void Sleep(unsigned int ms) { std::this_thread::sleep_for(std::chrono::milliseconds(ms)); }

struct AVCodec { const char *name; };
struct AVPacket { int released = 0; };
struct AVFrame {};
struct AVCodecContext {
    const AVCodec *codec;
    void *priv_data;
    int send_calls = 0, receive_calls = 0, packets = 2;
    int send_again = 1, receive_error = 0, drain_delay_ms = 3;
    bool drain = false, eof = false, nonblocking = false;
    std::chrono::steady_clock::time_point eof_at{};
};
int av_opt_set_int(void *opaque, const char *name, int value, int) {
    auto *codec = static_cast<AVCodecContext *>(opaque);
    assert(!strcmp(name, "query_timeout") && value == 0);
    codec->nonblocking = true;
    return 0;
}
int avcodec_send_frame(AVCodecContext *codec, AVFrame *frame) {
    assert(!frame);
    ++codec->send_calls;
    if (codec->send_again-- > 0)
        return AVERROR(EAGAIN);
    codec->drain = true;
    codec->eof_at = std::chrono::steady_clock::now() +
                    std::chrono::milliseconds(codec->drain_delay_ms);
    return 0;
}
int avcodec_receive_packet(AVCodecContext *codec, AVPacket *) {
    ++codec->receive_calls;
    if (codec->receive_error)
        return codec->receive_error;
    if (codec->packets > 0) {
        --codec->packets;
        return 0;
    }
    if (!codec->drain || std::chrono::steady_clock::now() < codec->eof_at)
        return AVERROR(EAGAIN);
    codec->eof = true;
    return AVERROR_EOF;
}
void av_packet_unref(AVPacket *packet) { ++packet->released; }

struct ID3D11Query {};
struct ID3D11DeviceContext;
template<class T> class ComPtr {
    T *value = nullptr;
public:
    ~ComPtr() {
        if constexpr (std::is_same_v<T, ID3D11Query>)
            delete value;
    }
    T *Get() const { return value; }
    T **GetAddressOf() { return &value; }
    explicit operator bool() const { return value != nullptr; }
};
constexpr int D3D11_QUERY_EVENT = 0, D3D11_ASYNC_GETDATA_DONOTFLUSH = 1;
struct D3D11_QUERY_DESC { int Query, MiscFlags; };
struct ID3D11Device {
    ID3D11DeviceContext *context = nullptr;
    HRESULT create_error = S_OK, removed = S_OK;
    int queries = 0;
    HRESULT CreateQuery(D3D11_QUERY_DESC *desc, ID3D11Query **query) {
        assert(desc->Query == D3D11_QUERY_EVENT);
        ++queries;
        if (FAILED(create_error))
            return create_error;
        *query = new ID3D11Query;
        return S_OK;
    }
    HRESULT GetDeviceRemovedReason() { return removed; }
    void GetImmediateContext(ID3D11DeviceContext **out) { *out = context; }
};
struct ID3D11DeviceContext {
    int delay_ms = 0, ends = 0, polls = 0, flushes = 0, pending_polls = 0;
    bool complete = false, false_forever = false;
    HRESULT poll_error = S_OK;
    std::chrono::steady_clock::time_point ready_at{};
    void End(ID3D11Query *) {
        ++ends;
        ready_at = std::chrono::steady_clock::now() + std::chrono::milliseconds(delay_ms);
    }
    void Flush() { ++flushes; }
    HRESULT GetData(ID3D11Query *, BOOL *done, size_t length, int flags) {
        assert(length == sizeof(BOOL) && flags == D3D11_ASYNC_GETDATA_DONOTFLUSH);
        ++polls;
        if (FAILED(poll_error))
            return poll_error;
        if (pending_polls > 0) {
            --pending_polls;
            *done = TRUE; // S_FALSE leaves output undefined; TRUE still cannot prove idle.
            return S_FALSE;
        }
        complete = !false_forever && std::chrono::steady_clock::now() >= ready_at;
        *done = complete ? TRUE : FALSE;
        return S_OK; // S_OK alone is deliberately not a completion witness.
    }
};
// Windows SDK headers expose these function-like macros in the native build.
// Include the STL first, then keep macro pollution active while parsing the real header.
#define min(left, right) ((left) < (right) ? (left) : (right))
#define max(left, right) ((left) > (right) ? (left) : (right))
#include "gpu_retirement.h"
#undef min
#undef max

static AVCodec amf{"hevc_amf"};
static AVCodecContext codec() {
    AVCodecContext value{&amf, nullptr};
    return value;
}
static void eof_is_required_and_eagain_is_retryable(void) {
    // The production deadline must compile with SDK macros and retain both clamps.
    auto before = std::chrono::steady_clock::now();
    auto zero = gpu_retirement_deadline(-1);
    auto after = std::chrono::steady_clock::now();
    assert(zero >= before && zero <= after);
    before = std::chrono::steady_clock::now();
    auto capped = gpu_retirement_deadline(2000);
    after = std::chrono::steady_clock::now();
    assert(capped >= before + std::chrono::milliseconds(1500));
    assert(capped <= after + std::chrono::milliseconds(1500));
    auto state = codec();
    state.priv_data = &state;
    AVPacket packet;
    bool sent = false;
    assert(drain_encoder_for_retirement(&state, &packet, sent,
                                        gpu_retirement_deadline(100)) == 0);
    assert(state.nonblocking && sent && state.eof);
    assert(state.send_calls == 2 && packet.released == 2);
    assert(state.receive_calls > packet.released); // Empty pending-count is not EOF.
}
static void one_deadline_covers_drain_and_both_devices(void) {
    auto state = codec();
    state.priv_data = &state;
    state.drain_delay_ms = 25;
    AVPacket packet;
    bool sent = false;
    ID3D11DeviceContext capture, encode;
    capture.delay_ms = 20;
    encode.delay_ms = 80;
    ID3D11Device capture_device, encode_device;
    capture_device.context = &capture;
    encode_device.context = &encode;
    ComPtr<ID3D11Query> capture_event, encode_event;
    auto begin = std::chrono::steady_clock::now();
    auto deadline = gpu_retirement_deadline(80);
    assert(drain_encoder_for_retirement(&state, &packet, sent, deadline) == 0);
    assert(wait_context_for_retirement(&capture_device, &capture, capture_event, deadline) == 0);
    assert(wait_context_for_retirement(&encode_device, &encode, encode_event, deadline) == AVERROR(ETIMEDOUT));
    auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::steady_clock::now() - begin).count();
    assert(elapsed >= 70 && elapsed < 500);
    assert(capture.complete && !encode.complete);
    assert(capture_event && encode_event); // Failed phase's owner still retains its query.
    assert(encode.ends == 1 && encode_device.queries == 1);
}
static void no_flush_or_sok_false_can_claim_gpu_idle(void) {
    ID3D11DeviceContext context;
    ID3D11Device device;
    device.context = &context;
    context.false_forever = true;
    ComPtr<ID3D11Query> event;
    assert(wait_context_for_retirement(&device, &context, event,
                                       gpu_retirement_deadline(3)) == AVERROR(ETIMEDOUT));
    assert(context.flushes == 1 && !context.complete && event);
    ID3D11DeviceContext pending;
    pending.pending_polls = 2;
    ID3D11Device pending_device;
    pending_device.context = &pending;
    ComPtr<ID3D11Query> pending_event;
    assert(wait_context_for_retirement(&pending_device, &pending, pending_event,
                                       gpu_retirement_deadline(100)) == 0);
    assert(pending.polls == 3 && pending.complete && pending.flushes == 1);
    assert(pending_device.queries == 1 && pending.ends == 1);
    device.removed = static_cast<HRESULT>(0x887A0005u);
    assert(wait_context_for_retirement(&device, &context, event,
                                       gpu_retirement_deadline(10)) == device.removed);
    assert(device.queries == 1 && context.ends == 1); // No replacement/extra queued query.
    auto state = codec();
    state.priv_data = &state;
    state.receive_error = AVERROR(EIO);
    AVPacket packet;
    bool sent = false;
    assert(drain_encoder_for_retirement(&state, &packet, sent,
                                        gpu_retirement_deadline(10)) == AVERROR(EIO));
}
int main(void) {
    eof_is_required_and_eagain_is_retryable();
    one_deadline_covers_drain_and_both_devices();
    no_flush_or_sok_false_can_claim_gpu_idle();
    puts("3 CPU mock GPU retirement control-flow/deadline/error tests passed");
    return 0;
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cxx', default='c++')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    temporary_root = Path(tempfile.gettempdir()).resolve()
    with tempfile.TemporaryDirectory(prefix='xscc-gpu-retirement-', dir=temporary_root) as directory:
        work = Path(directory).resolve()
        if work.parent != temporary_root:
            raise ValueError('Retirement mock directory escaped the temporary root')
        source = work / 'test.cpp'
        source.write_text(TEST)
        executable = work / ('test.exe' if os.name == 'nt' else 'test')
        runtime = ['-fms-runtime-lib=static'] if os.name == 'nt' else []
        subprocess.run(
            [args.cxx, '-std=c++17', '-Wall', '-Wextra', '-Werror', *runtime,
             '-I' + str(root / 'vendor/hwcodec/cpp/common'),
             str(source), '-o', str(executable)],
            check=True, timeout=120,
        )
        subprocess.run([str(executable)], check=True, timeout=10)


if __name__ == '__main__':
    main()
