package com.radishledger.radishledger_probe

import android.app.Activity
import android.content.pm.ApplicationInfo
import android.os.Build
import android.os.Process
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File

/** 仅回报本项目合成文件的元数据；不读取文件载荷、不请求系统认证。 */
object M0FileBoundary {
    private val ids = Regex("^[0-9a-f]{32}$")
    private val names = setOf("owner", "lock", "ledger.db", "ledger.db-wal", "ledger.db-shm", "auxiliary.db", "scanner-control.db")

    private fun metadata(file: File, directory: Boolean): Map<String, Any> {
        val info = Os.lstat(file.path)
        check(if (directory) OsConstants.S_ISDIR(info.st_mode) else OsConstants.S_ISREG(info.st_mode))
        val label = try {
            mapOf("value" to Os.getxattr(file.path, "security.selinux").toString(Charsets.UTF_8).trimEnd('\u0000'), "errno" to 0)
        } catch (error: ErrnoException) {
            mapOf("errno" to error.errno)
        }
        return mapOf("path" to file.canonicalPath, "mode" to (info.st_mode and 511),
            "uid" to info.st_uid, "bytes" to info.st_size, "selinux" to label)
    }

    fun environment(activity: Activity, storage: File): Map<String, Any> {
        val app = activity.applicationInfo
        return mapOf("platform" to "android", "pid" to Process.myPid(), "uid" to Process.myUid(),
            "sdk" to Build.VERSION.SDK_INT, "targetSdk" to app.targetSdkVersion,
            "debuggable" to (app.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0),
            "allowBackup" to (app.flags and ApplicationInfo.FLAG_ALLOW_BACKUP != 0),
            "selinux" to File("/proc/self/attr/current").readText().trim().trimEnd('\u0000'),
            "storage" to storage.canonicalPath, "storageMetadata" to metadata(storage, true),
            "noBackup" to activity.noBackupFilesDir.canonicalPath,
            "cache" to activity.cacheDir.canonicalPath, "codeCache" to activity.codeCacheDir.canonicalPath,
            "data" to File(app.dataDir).canonicalPath, "apk" to app.sourceDir,
            "nativeLibraryDirectory" to app.nativeLibraryDir)
    }

    fun attributes(storage: File, run: String): Map<String, Any> {
        require(ids.matches(run))
        val root = File(storage.canonicalFile, "m0c-$run")
        val rootInfo = metadata(root, true)
        check(root.canonicalFile.parentFile == storage.canonicalFile)
        val entries = root.listFiles() ?: error("synthetic directory unavailable")
        check(entries.map { it.name }.toSet() == names)
        return mapOf("root" to rootInfo, "files" to entries.sortedBy { it.name }.map { metadata(it, false) })
    }
}
