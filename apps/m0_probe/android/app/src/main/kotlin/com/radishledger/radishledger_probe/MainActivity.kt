package com.radishledger.radishledger_probe

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private var protection: M0Protection? = null
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        protection = M0Protection(this, MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "radishledger/m0c/protection"))
    }
    override fun onStop() { protection?.cancel(); super.onStop() }
    override fun onDestroy() { protection?.close(); super.onDestroy() }
}
