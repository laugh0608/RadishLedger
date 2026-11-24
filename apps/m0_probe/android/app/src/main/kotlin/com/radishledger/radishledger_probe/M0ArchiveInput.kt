package com.radishledger.radishledger_probe

import android.app.Activity
import android.content.Intent
import android.net.Uri
import io.flutter.plugin.common.MethodChannel
import java.io.DataInputStream
import java.io.File
import java.io.IOException
import java.util.UUID

/** 只保留本次系统选择授予的只读 URI，私有缓存中只接收有界的归档候选。 */
class M0ArchiveInput(private val activity: Activity) {
    private val requestCode = 8134
    private var picker: MethodChannel.Result? = null
    @Volatile private var selected: Uri? = null

    fun choose(result: MethodChannel.Result) {
        if (picker != null) {
            result.error("restore_busy", "正在选择归档", null)
            return
        }
        selected = null
        picker = result
        try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                putExtra(Intent.EXTRA_LOCAL_ONLY, true)
                type = "*/*"
            }
            activity.startActivityForResult(intent, requestCode)
        } catch (error: Exception) {
            picker = null
            result.error("archive_picker_unavailable", "无法打开本机文件选择器", error.javaClass.simpleName)
        }
    }

    fun selected(code: Int, status: Int, data: Intent?): Boolean {
        if (code != requestCode) return false
        val result = picker ?: return true
        picker = null
        val uri = data?.data
        when {
            status == Activity.RESULT_CANCELED -> result.success(false)
            status != Activity.RESULT_OK || uri?.scheme != "content" ->
                result.error("invalid_archive_input", "未取得可读文档", null)
            else -> { selected = uri; result.success(true) }
        }
        return true
    }

    fun close() {
        picker?.success(false)
        picker = null
        selected = null
    }

    /** 在串行后台通道复制，绝不把文件提供方给出的名字用作本机路径。 */
    fun copy(limit: Int, magic: ByteArray, result: MethodChannel.Result) {
        val uri = selected
        selected = null
        if (uri == null || limit <= 0 || magic.isEmpty()) {
            result.error("invalid_archive_input", "需要重新选择文件", null)
            return
        }
        val parent = activity.cacheDir.canonicalFile
        val dir = File(parent, "archive-" + UUID.randomUUID().toString().replace("-", ""))
        val source = File(dir, "source.rla")
        var owned = false
        try {
            val stream = activity.contentResolver.openInputStream(uri) ?: throw IOException()
            DataInputStream(stream).use { input ->
                val head = ByteArray(magic.size)
                input.readFully(head)
                if (!head.contentEquals(magic)) {
                    result.error("invalid_archive", "归档文件头无效", null)
                    return
                }
                if (!dir.mkdir()) throw IOException()
                owned = true
                source.outputStream().use { target ->
                    target.write(head)
                    val buffer = ByteArray(64 * 1024)
                    var total = head.size.toLong()
                    while (true) {
                        val n = input.read(buffer)
                        if (n < 0) break
                        total += n
                        if (n == 0 || total > limit) throw InputLimit()
                        target.write(buffer, 0, n)
                    }
                }
            }
            result.success(mapOf("directory" to parent.path, "stage" to dir.name))
        } catch (error: Exception) {
            val clean = !owned || ((!source.exists() || source.delete()) && dir.delete())
            val code = when {
                !clean -> "archive_cleanup_failed"
                error is InputLimit -> "archive_limit"
                error is java.io.EOFException -> "invalid_archive"
                else -> "archive_read_failed"
            }
            result.error(code, "读取归档失败", error.javaClass.simpleName)
        }
    }

    private class InputLimit : IOException()
}
