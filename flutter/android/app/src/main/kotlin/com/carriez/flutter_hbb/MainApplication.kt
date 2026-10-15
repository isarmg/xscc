package com.carriez.flutter_hbb

import android.app.Application
import android.content.ClipboardManager
import android.content.Context
import android.util.Log
import ffi.FFI

class MainApplication : Application() {
    companion object {
        private const val TAG = "MainApplication"
    }

    lateinit var rdClipboardManager: RdClipboardManager
        private set

    override fun onCreate() {
        super.onCreate()
        Log.d(TAG, "App start")
        FFI.onAppStart(applicationContext)
        rdClipboardManager = RdClipboardManager(
            applicationContext.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        )
        FFI.setClipboardManager(rdClipboardManager)
        FFI.setCodecInfo(getHardwareDecoderInfo())
    }
}
