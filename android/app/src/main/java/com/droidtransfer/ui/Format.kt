package com.droidtransfer.ui

import android.content.Context
import android.net.Uri
import android.util.Log

internal object Format {
    fun bytes(b: Long): String {
        if (b < 1024) return "$b B"
        val units = arrayOf("KB", "MB", "GB", "TB")
        var v = b / 1024.0
        var i = 0
        while (v >= 1024 && i < units.size - 1) {
            v /= 1024
            i++
        }
        return String.format("%.1f %s", v, units[i])
    }

    fun duration(secs: Long): String = when {
        secs < 60 -> "${secs}s"
        secs < 3600 -> "${secs / 60}m ${secs % 60}s"
        else -> "${secs / 3600}h ${(secs % 3600) / 60}m"
    }

    fun ago(atMillis: Long): String {
        val s = (System.currentTimeMillis() - atMillis) / 1000
        return when {
            s < 5 -> "just now"
            s < 60 -> "${s}s ago"
            s < 3600 -> "${s / 60}m ago"
            s < 86400 -> "${s / 3600}h ago"
            else -> "${s / 86400}d ago"
        }
    }

    fun code(code: String): String = if (code.length == 6) code.substring(0, 3) + " " + code.substring(3) else code

    fun isUrl(s: String): Boolean {
        val t = s.trim()
        return (t.startsWith("http://") || t.startsWith("https://")) && t.none { it.isWhitespace() }
    }
}

/**
 * Temporary URI grants (share sheet, document picker) die with the activity that received them.
 * Transfers run in the service after the activity is gone, so pin the grant to our own package.
 */
internal fun Context.keepReadAccess(uri: Uri, persistable: Boolean = false) {
    try {
        if (persistable) {
            contentResolver.takePersistableUriPermission(uri, android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } else {
            grantUriPermission(packageName, uri, android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    } catch (e: Exception) {
        Log.w("DroidTransfer", "Could not keep access to $uri: ${e.message}")
    }
}
