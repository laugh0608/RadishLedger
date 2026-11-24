package com.radishledger.radishledger_probe

import android.content.Intent
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.StandardMethodCodec
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private val archiveInput = M0ArchiveInput(this)
    private var protection: M0Protection? = null
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val messenger = flutterEngine.dartExecutor.binaryMessenger
        MethodChannel(messenger, "radishledger/internal/archive", StandardMethodCodec.INSTANCE,
            messenger.makeBackgroundTaskQueue()).setMethodCallHandler(M0ArchiveDownloads(applicationContext))
        MethodChannel(messenger, "radishledger/internal/archive-picker").setMethodCallHandler { call, result ->
            if (call.method == "choose") archiveInput.choose(result) else result.notImplemented()
        }
        MethodChannel(messenger, "radishledger/internal/archive-input", StandardMethodCodec.INSTANCE,
            messenger.makeBackgroundTaskQueue()).setMethodCallHandler { call, result ->
            if (call.method == "copy") archiveInput.copy(call.argument<Int>("limit") ?: 0,
                call.argument<ByteArray>("magic") ?: byteArrayOf(), result) else result.notImplemented()
        }
        protection = M0Protection(this, MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "radishledger/m0c/protection"))
    }
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (!archiveInput.selected(requestCode, resultCode, data)) super.onActivityResult(requestCode, resultCode, data)
    }
    override fun onStop() { protection?.cancel(); super.onStop() }
    override fun onDestroy() { archiveInput.close(); protection?.close(); super.onDestroy() }
}
