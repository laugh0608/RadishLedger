package com.radishledger.radishledger_probe

import android.app.Activity
import android.app.KeyguardManager
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.UserNotAuthenticatedException
import android.system.Os
import android.system.OsConstants
import android.system.ErrnoException
import android.util.AtomicFile
import android.util.Base64
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.security.KeyStore
import java.util.concurrent.Executors
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec

/** M0-C 精确 alias 适配；不允许静默去掉认证或改用可导出软件钥。 */
class M0Protection(private val activity: Activity, private val channel: MethodChannel) {
    private val worker = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())
    private val ids = Regex("^[0-9a-f]{32}$")
    private var pending: CancellationSignal? = null
    private var busy = false
    @Volatile private var epoch = 0
    private val authenticators = BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL

    init { channel.setMethodCallHandler(::handle) }
    fun cancel() { epoch += 1; pending?.cancel() }
    fun close() { cancel(); channel.setMethodCallHandler(null); worker.shutdown() }

    private class Failure(val code: String, val platformCode: String = "") : Exception()
    private class Operation(val result: MethodChannel.Result, val run: String, val generation: String,
                            val method: String, val secret: ByteArray, val epoch: Int) {
        val alias = "radishledger.m0c.$run.$generation"
        var created = false
    }
    private fun keys(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private fun storage(name: String): File {
        val file = File(activity.noBackupFilesDir, name)
        if (!file.isDirectory && !file.mkdirs()) throw IOException("private directory")
        if (!file.setReadable(false, false) || !file.setWritable(false, false) || !file.setExecutable(false, false)
            || !file.setReadable(true, true) || !file.setWritable(true, true) || !file.setExecutable(true, true)) throw IOException("private permissions")
        return file
    }
    private fun wrapped(op: Operation): File = File(storage("m0c-protection"), "${op.run}.${op.generation}.json")
    private fun aad(op: Operation) = "RadishLedger/M0C/Android/v1/${op.run}/${op.generation}".toByteArray(Charsets.UTF_8)
    private fun capabilities(): Map<String, Any> {
        val secure = activity.getSystemService(KeyguardManager::class.java).isDeviceSecure
        val status = if (Build.VERSION.SDK_INT >= 30) activity.getSystemService(BiometricManager::class.java).canAuthenticate(authenticators) else -1
        return mapOf("supported" to (Build.VERSION.SDK_INT >= 30 && secure && status == BiometricManager.BIOMETRIC_SUCCESS),
            "deviceSecure" to secure, "api" to Build.VERSION.SDK_INT, "platformCode" to status,
            "protection" to "androidKeystore+Aes256Gcm+perUseAuthentication", "hardwareClaim" to "not-measured")
    }
    private fun mapped(error: Exception): Failure = when (error) {
        is Failure -> error
        is KeyPermanentlyInvalidatedException -> Failure("item_invalidated", error.javaClass.simpleName)
        is UserNotAuthenticatedException -> Failure("authentication_required", error.javaClass.simpleName)
        is AEADBadTagException -> Failure("corrupt_protection", error.javaClass.simpleName)
        is IOException -> Failure("storage_unavailable", error.javaClass.simpleName)
        else -> Failure("platform_error", error.javaClass.simpleName)
    }
    private fun control(remove: Boolean): String {
        val root = storage("m0-c")
        for (name in listOf("control-in", "control-out")) {
            val path = File(root, name).path
            try {
                val info = Os.lstat(path)
                if (!OsConstants.S_ISFIFO(info.st_mode)) throw Failure("control_unavailable")
                if (remove && !File(path).delete()) throw Failure("control_unavailable")
            } catch (error: ErrnoException) {
                if (error.errno != OsConstants.ENOENT) throw error
                if (!remove) Os.mkfifo(path, 384)
            }
        }
        if (remove) {
            val ready = File(root, "control-ready.json")
            if (ready.exists() && !ready.delete()) throw Failure("control_unavailable")
        }
        return root.path
    }
    private fun handle(call: MethodCall, result: MethodChannel.Result) {
        if (call.method == "control" || call.method == "removeControl") {
            try { result.success(control(call.method == "removeControl")) }
            catch (error: Exception) { val failure = mapped(error); result.error(failure.code, "实验管道不可用", mapOf("platformCode" to failure.platformCode)) }
            return
        }
        if (call.method == "cancel") { cancel(); result.success(null); return }
        if (call.method == "capabilities") { result.success(capabilities()); return }
        if (call.method == "storage") {
            try { result.success(storage("m0-c").absolutePath) }
            catch (error: Exception) { val failure = mapped(error); result.error(failure.code, "实验私有目录不可用", mapOf("platformCode" to failure.platformCode)) }
            return
        }
        val run = call.argument<String>("run")
        val generation = call.argument<String>("generation")
        val secret = call.argument<ByteArray>("secret") ?: ByteArray(0)
        if (call.method !in listOf("create", "read", "delete") || run == null || generation == null || !ids.matches(run) || !ids.matches(generation)
            || (call.method == "create" && secret.size != 32)) {
            secret.fill(0); result.error("invalid_argument", "系统保护参数无效", null); return
        }
        if (busy) { secret.fill(0); result.error("protection_busy", "已有系统保护操作尚未结束", null); return }
        busy = true
        val op = Operation(result, run, generation, call.method, secret, epoch)
        worker.execute {
            try {
                if (op.method == "delete") {
                    val existed = keys().containsAlias(op.alias)
                    remove(op)
                    finish(op, mapOf("deleted" to true, "alreadyMissing" to !existed), null)
                    return@execute
                }
                if (Build.VERSION.SDK_INT < 30) throw Failure("unsupported")
                val store = keys()
                val cipher = Cipher.getInstance("AES/GCM/NoPadding")
                if (op.method == "create") {
                    if (capabilities()["supported"] != true) throw Failure("unsupported")
                    if (store.containsAlias(op.alias) || wrapped(op).exists()) throw Failure("item_exists")
                    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
                    generator.init(KeyGenParameterSpec.Builder(op.alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setKeySize(256).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setRandomizedEncryptionRequired(true).setUserAuthenticationRequired(true)
                        .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL).build())
                    val key = generator.generateKey()
                    op.created = true
                    cipher.init(Cipher.ENCRYPT_MODE, key)
                } else {
                    if (!store.containsAlias(op.alias)) throw Failure("item_missing")
                    val key = store.getKey(op.alias, null) as? SecretKey ?: throw Failure("item_unavailable")
                    val file = wrapped(op)
                    if (!file.isFile) throw Failure("material_missing")
                    if (file.length() > 1024) throw Failure("corrupt_protection")
                    val document = JSONObject(file.readText(Charsets.UTF_8))
                    if (document.getInt("format") != 1 || document.getString("run") != op.run || document.getString("generation") != op.generation) throw Failure("corrupt_protection")
                    val iv = Base64.decode(document.getString("iv"), Base64.NO_WRAP)
                    if (iv.size != 12) throw Failure("corrupt_protection")
                    cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv))
                }
                cipher.updateAAD(aad(op))
                main.post { authenticate(op, cipher) }
            } catch (error: Exception) { finish(op, null, mapped(error)) }
        }
    }
    private fun authenticate(op: Operation, cipher: Cipher) {
        if (op.epoch != epoch || activity.isFinishing) { worker.execute { finish(op, null, Failure("interrupted")) }; return }
        val signal = CancellationSignal()
        pending = signal
        try {
            BiometricPrompt.Builder(activity).setTitle("解锁合成实验账库")
                .setSubtitle("系统认证仅用于本次 RadishLedger 保护项")
                .setAllowedAuthenticators(authenticators).build()
                .authenticate(BiometricPrompt.CryptoObject(cipher), signal, activity.mainExecutor, object : BiometricPrompt.AuthenticationCallback() {
                    override fun onAuthenticationError(code: Int, message: CharSequence?) {
                        val category = when (code) {
                            BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED -> "user_cancelled"
                            BiometricPrompt.BIOMETRIC_ERROR_CANCELED -> "interrupted"
                            BiometricPrompt.BIOMETRIC_ERROR_LOCKOUT, BiometricPrompt.BIOMETRIC_ERROR_LOCKOUT_PERMANENT -> "authentication_denied"
                            BiometricPrompt.BIOMETRIC_ERROR_HW_UNAVAILABLE -> "temporarily_unavailable"
                            else -> "authentication_unavailable"
                        }
                        worker.execute { finish(op, null, Failure(category, code.toString())) }
                    }
                    override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                        worker.execute {
                            try {
                                if (op.epoch != epoch) throw Failure("interrupted")
                                val authenticated = result.cryptoObject?.cipher ?: throw Failure("authentication_denied")
                                if (op.method == "create") {
                                    val encrypted = authenticated.doFinal(op.secret)
                                    val bytes = JSONObject().put("format", 1).put("run", op.run).put("generation", op.generation)
                                        .put("iv", Base64.encodeToString(authenticated.iv, Base64.NO_WRAP))
                                        .put("ciphertext", Base64.encodeToString(encrypted, Base64.NO_WRAP)).toString().toByteArray(Charsets.UTF_8)
                                    val file = AtomicFile(wrapped(op))
                                    val output = file.startWrite()
                                    try { output.write(bytes); output.fd.sync(); file.finishWrite(output) }
                                    catch (error: Exception) { file.failWrite(output); throw error }
                                    val key = keys().getKey(op.alias, null) as SecretKey
                                    val info = SecretKeyFactory.getInstance(key.algorithm, "AndroidKeyStore").getKeySpec(key, KeyInfo::class.java) as KeyInfo
                                    val level = if (Build.VERSION.SDK_INT >= 31) info.securityLevel.toString() else "unmeasured"
                                    finish(op, mapOf("created" to true, "securityLevelCode" to level), null)
                                } else {
                                    val document = JSONObject(wrapped(op).readText(Charsets.UTF_8))
                                    val encrypted = Base64.decode(document.getString("ciphertext"), Base64.NO_WRAP)
                                    if (encrypted.size != 48) throw Failure("corrupt_protection")
                                    val clear = authenticated.doFinal(encrypted)
                                    if (clear.size != 32) { clear.fill(0); throw Failure("corrupt_protection") }
                                    finish(op, clear, null)
                                }
                            } catch (error: Exception) { finish(op, null, mapped(error)) }
                        }
                    }
                })
        } catch (error: Exception) { worker.execute { finish(op, null, mapped(error)) } }
    }
    private fun remove(op: Operation) {
        val store = keys()
        if (store.containsAlias(op.alias)) store.deleteEntry(op.alias)
        val file = wrapped(op)
        if (file.exists() && !file.delete()) throw IOException("exact item cleanup")
        // AtomicFile 只管理同一个已知目标的辅助文件。
        for (suffix in listOf(".bak", ".new")) {
            val extra = File(file.path + suffix)
            if (extra.exists() && !extra.delete()) throw IOException("exact item cleanup")
        }
    }
    private fun finish(op: Operation, value: Any?, error: Failure?) {
        var failure = error
        if (op.epoch != epoch && failure == null) failure = Failure("interrupted")
        if (failure != null && op.created) {
            try { remove(op) } catch (cleanup: Exception) { failure = Failure("cleanup_failed", cleanup.javaClass.simpleName) }
        }
        op.secret.fill(0)
        val finalFailure = failure
        main.post {
            pending = null; busy = false
            if (finalFailure != null) op.result.error(finalFailure.code, "系统保护操作未完成，原账库保留", mapOf("platformCode" to finalFailure.platformCode))
            else if (op.epoch != epoch) op.result.error("interrupted", "操作已因生命周期变化结束", null)
            else op.result.success(value)
            // Flutter codec 已同步编码；其内部副本不能承诺清零。
            if (value is ByteArray) value.fill(0)
        }
    }
}
