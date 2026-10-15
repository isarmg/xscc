package com.carriez.flutter_hbb

import ffi.FFI
import kotlin.concurrent.thread

import android.util.Log
import android.view.KeyEvent
import android.view.WindowManager
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.android.FlutterView
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private val channelTag = "mChannel"
    private val logTag = "mMainActivity"

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val flutterMethodChannel = MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            channelTag
        )
        initFlutterChannel(flutterMethodChannel)
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.action == KeyEvent.ACTION_DOWN &&
            KeyEvent.isModifierKey(event.keyCode) && currentFocus == null) {
            // Android can drop modifier-first input when the native view has no focus.
            findViewById<FlutterView>(FlutterActivity.FLUTTER_VIEW_ID)?.requestFocus()
        }
        return super.dispatchKeyEvent(event)
    }

    override fun onDestroy() {
        Log.e(logTag, "onDestroy")
        // Activity recreation must not tear down a live outgoing session.
        if (isFinishing) {
            FFI.closeAllSessions()
        }
        super.onDestroy()
    }

    private fun initFlutterChannel(flutterMethodChannel: MethodChannel) {
        flutterMethodChannel.setMethodCallHandler { call, result ->
            // make sure result will be invoked, otherwise flutter will await forever
            when (call.method) {
                "enable_soft_keyboard" -> {
                    // https://blog.csdn.net/hanye2020/article/details/105553780
                    if (call.arguments as Boolean) {
                        window.clearFlags(WindowManager.LayoutParams.FLAG_ALT_FOCUSABLE_IM)
                    } else {
                        window.addFlags(WindowManager.LayoutParams.FLAG_ALT_FOCUSABLE_IM)
                    }
                    result.success(true)
                }
                "try_sync_clipboard" -> {
                    (application as MainApplication).rdClipboardManager.syncClipboard()
                    result.success(true)
                }
                else -> {
                    result.error("-1", "No such method", null)
                }
            }
        }
    }
}
