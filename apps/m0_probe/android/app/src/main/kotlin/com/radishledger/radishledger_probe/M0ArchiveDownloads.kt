package com.radishledger.radishledger_probe

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.IOException

/** 串行后台通道只处理本次新建的下载项。口令、账务和认证留在 Rust。 */
class M0ArchiveDownloads(private val context: Context) : MethodChannel.MethodCallHandler {
    private val pending = mutableMapOf<String, Uri>()
    private val resolver get() = context.contentResolver
    private val relative = Environment.DIRECTORY_DOWNLOADS + "/RadishLedger/"
    private class Failure(val code: String) : Exception()

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        if (Build.VERSION.SDK_INT < 29) {
            if (call.method == "discard") { result.success(null); return }
            result.error("archive_downloads_unavailable", "下载归档需要 Android 10 或以上", null)
            return
        }
        try {
            val stage = call.argument<String>("stage") ?: throw Failure("invalid_archive_stage")
            if (!Regex("archive-[a-zA-Z0-9]{6,32}").matches(stage)) throw Failure("invalid_archive_stage")
            when (call.method) {
                "prepare" -> {
                    val name = call.argument<String>("name") ?: throw Failure("invalid_archive_name")
                    if (!Regex("[\\p{L}\\p{N}_-]{1,48}\\.rla").matches(name)) throw Failure("invalid_archive_name")
                    prepare(stage, name)
                    result.success(null)
                }
                "publish" -> {
                    // 从这一点起不再删除：提供方可能已发布但回执丢失。
                    val uri = pending.remove(stage) ?: throw Failure("archive_target_missing")
                    try {
                        val values = ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }
                        if (resolver.update(uri, values, null, null) != 1) throw IOException()
                        val displayName = resolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use {
                            if (it.moveToFirst()) it.getString(0) else null
                        } ?: throw IOException()
                        result.success(displayName)
                    } catch (_: Exception) { throw Failure("archive_publish_uncertain") }
                }
                "discard" -> {
                    val uri = pending[stage]
                    if (uri != null) {
                        if (resolver.delete(uri, null, null) != 1) throw Failure("archive_cleanup_failed")
                        pending.remove(stage)
                    }
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        } catch (error: Exception) {
            result.error((error as? Failure)?.code ?: "archive_download_io", "归档文件操作未完成", null)
        }
    }

    @android.annotation.TargetApi(29)
    private fun prepare(stage: String, name: String) {
        val parent = File(context.noBackupFilesDir, "internal-ledger").canonicalFile
        val dir = File(parent, stage)
        val source = File(dir, "sealed")
        if (dir.canonicalFile != dir.absoluteFile || !source.isFile || source.canonicalFile != source.absoluteFile ||
            pending.containsKey(stage)) throw Failure("invalid_archive_stage")
        val downloads = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        val exists = resolver.query(downloads, arrayOf(MediaStore.Downloads._ID),
            "${MediaStore.MediaColumns.RELATIVE_PATH}=? AND ${MediaStore.MediaColumns.DISPLAY_NAME}=?",
            arrayOf(relative, name), null)?.use { it.moveToFirst() } ?: throw IOException()
        if (exists) throw Failure("archive_already_exists")
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.RELATIVE_PATH, relative)
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, "application/octet-stream")
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(downloads, values) ?: throw IOException()
        pending[stage] = uri
        val descriptor = resolver.openFileDescriptor(uri, "w") ?: throw IOException()
        ParcelFileDescriptor.AutoCloseOutputStream(descriptor).use { output ->
            source.inputStream().use { it.copyTo(output) }
            output.fd.sync()
        }
        val length = source.length()
        resolver.openInputStream(uri)?.use { input ->
            val back = File(dir, "readback")
            if (!back.createNewFile()) throw Failure("invalid_archive_stage")
            back.outputStream().use { output ->
                val chunk = ByteArray(64 * 1024)
                var total = 0L
                while (true) {
                    val count = input.read(chunk)
                    if (count < 0) break
                    total += count
                    if (count == 0 || total > length) throw Failure("archive_file_mismatch")
                    output.write(chunk, 0, count)
                }
                if (total != length) throw Failure("archive_file_mismatch")
            }
        } ?: throw IOException()
    }
}
