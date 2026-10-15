package com.carriez.flutter_hbb

import android.media.MediaCodecInfo.CodecCapabilities.FEATURE_SecurePlayback
import android.media.MediaCodecInfo.CodecCapabilities.FEATURE_TunneledPlayback
import android.media.MediaCodecList
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject

fun getHardwareDecoderInfo(): String {
    val decoders = JSONArray()
    val codecs = try {
        MediaCodecList(MediaCodecList.ALL_CODECS).codecInfos
    } catch (e: Exception) {
        Log.e("HardwareDecoderInfo", "Failed to enumerate video decoders", e)
        return decoders.toString()
    }
    for (codec in codecs) {
        if (codec.isEncoder || !codec.isHardwareAccelerated) {
            continue
        }
        for (mimeType in codec.supportedTypes) {
            if (mimeType != "video/avc" && mimeType != "video/hevc") {
                continue
            }
            try {
                val caps = codec.getCapabilitiesForType(mimeType)
                if (caps.isFeatureRequired(FEATURE_SecurePlayback) ||
                    caps.isFeatureRequired(FEATURE_TunneledPlayback)) {
                    continue
                }
                decoders.put(JSONObject().apply {
                    put("name", codec.name)
                    put("mime_type", mimeType)
                    put("color_formats", JSONArray(caps.colorFormats.toList()))
                })
            } catch (e: Exception) {
                Log.e("HardwareDecoderInfo", "Failed to inspect ${codec.name}: $mimeType", e)
            }
        }
    }
    return decoders.toString()
}
