package com.carriez.flutter_hbb

import java.nio.ByteBuffer
import java.util.Timer
import java.util.TimerTask

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.util.Log
import androidx.annotation.Keep

import hbb.MessageOuterClass.ClipboardFormat
import hbb.MessageOuterClass.Clipboard
import hbb.MessageOuterClass.MultiClipboards

import ffi.FFI

class RdClipboardManager(private val clipboardManager: ClipboardManager) {
    private val logTag = "RdClipboardManager"
    // 1. Avoid listening to the same clipboard data updated by `rustUpdateClipboard`.
    // 2. Avoid sending the clipboard data before enabling client clipboard.
    //    1) Disable clipboard
    //    2) Copy text "a"
    //    3) Enable clipboard
    //    4) Switch to another app
    //    5) Switch back to the app
    //    6) "a" should not be sent to the client, because it's copied before enabling clipboard
    //
    // It's okay to that `rustEnableClientClipboard(false)` is called after `rustUpdateClipboard`,
    // though the `lastUpdatedClipData` will be set to null once.
    private var lastUpdatedClipData: ClipData? = null
    private var isClientEnabled = true;
    private fun checkPrimaryClip() {
        val clipData = clipboardManager.primaryClip
        if (clipData != null && clipData.itemCount > 0) {
            // Only handle the first item in the clipboard for now.
            val clip = clipData.getItemAt(0)
            if (lastUpdatedClipData != null && isClipboardDataEqual(clipData, lastUpdatedClipData!!)) {
                Log.d(logTag, "Clipboard data is the same as last update, ignore")
                return
            }
            val text = if (clipData.description.hasMimeType(ClipDescription.MIMETYPE_TEXT_PLAIN)) {
                clip.text
            } else {
                null
            }
            var count = 0
            val clips = MultiClipboards.newBuilder()
            if (text != null) {
                val content = com.google.protobuf.ByteString.copyFromUtf8(text.toString())
                clips.addClipboards(Clipboard.newBuilder().setFormat(ClipboardFormat.Text).setContent(content).build())
                count++
            }
            if (count > 0) {
                val clipsBytes = clips.build().toByteArray()
                val clipsBuf = ByteBuffer.allocateDirect(clipsBytes.size + 1).apply {
                    put(1.toByte())
                    put(clipsBytes)
                }
                clipsBuf.flip()
                lastUpdatedClipData = clipData
                Log.d(logTag, "client, send clipboard data to the remote")
                FFI.onClipboardUpdate(clipsBuf)
            }
        }
    }

    private fun isClipboardDataEqual(left: ClipData, right: ClipData): Boolean {
        if (left.description.getMimeTypeCount() != right.description.getMimeTypeCount()) {
            return false
        }
        val mimeTypeCount = left.description.getMimeTypeCount()
        for (i in 0 until mimeTypeCount) {
            if (left.description.getMimeType(i) != right.description.getMimeType(i)) {
                return false
            }
        }

        if (left.itemCount != right.itemCount) {
            return false
        }
        for (i in 0 until left.itemCount) {
            val leftItem = left.getItemAt(i)
            val rightItem = right.getItemAt(i)
            if (leftItem.text != rightItem.text) {
                return false
            }
        }
        return true
    }

    @Keep
    fun rustEnableClientClipboard(enable: Boolean) {
        Log.d(logTag, "rustEnableClientClipboard: enable: $enable")
        isClientEnabled = enable
        lastUpdatedClipData = null
    }

    fun syncClipboard() {
        Log.d(logTag, "syncClipboard: isClientEnabled: $isClientEnabled")
        if (!isClientEnabled) {
            return
        }
        checkPrimaryClip()
    }

    @Keep
    fun rustUpdateClipboard(clips: ByteArray) {
        val clips = MultiClipboards.parseFrom(clips)
        var text: String? = null
        for (clip in clips.getClipboardsList()) {
            if (clip.format == ClipboardFormat.Text) {
                text = String(clip.content.toByteArray(), Charsets.UTF_8)
            }
        }
        if (text == null) {
            return
        }
        val clipData = ClipData.newPlainText("clipboard", text)
        lastUpdatedClipData = clipData
        clipboardManager.setPrimaryClip(clipData)
    }
}
