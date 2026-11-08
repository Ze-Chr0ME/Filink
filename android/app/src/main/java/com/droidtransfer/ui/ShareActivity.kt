package com.droidtransfer.ui

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast
import com.droidtransfer.core.Hub
import com.droidtransfer.core.PcInfo

/** Target of Android's share sheet: hands the shared items to the service and gets out of the way. */
class ShareActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Hub.init(applicationContext)

        val uris = sharedUris(intent)
        val text = if (uris.isEmpty()) sharedText(intent) else null
        if (uris.isEmpty() && text.isNullOrBlank()) {
            toast("Nothing to send")
            return finish()
        }
        uris.forEach { keepReadAccess(it) }

        val paired = Hub.state.pcs.filter { it.paired }
        if (paired.isEmpty()) {
            toast("Pair DroidTransfer with your PC first")
            startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            return finish()
        }

        val default = paired.singleOrNull()
        if (default != null) {
            send(default, uris, text)
            return finish()
        }

        // Several PCs: online ones first, the last used one on top.
        val last = Hub.defaultPcId()
        val choices = paired.sortedWith(compareByDescending<PcInfo> { it.id == last }.thenByDescending { it.online })
        val labels = choices.map { if (it.online) it.name else "${it.name} (offline)" }.toTypedArray()
        AlertDialog.Builder(this)
            .setTitle("Send to")
            .setItems(labels) { _, which -> send(choices[which], uris, text) }
            .setOnDismissListener { finish() }
            .show()
    }

    private fun send(pc: PcInfo, uris: List<Uri>, text: String?) {
        if (uris.isNotEmpty()) {
            Hub.sendUris(pc.id, uris)
            toast(if (uris.size == 1) "Sending to ${pc.name}" else "Sending ${uris.size} files to ${pc.name}")
        } else if (text != null) {
            Hub.sendText(pc.id, text)
            toast("Text sent to ${pc.name}")
        }
    }

    private fun sharedUris(i: Intent): List<Uri> = when (i.action) {
        Intent.ACTION_SEND -> listOfNotNull(streamExtra(i))
        Intent.ACTION_SEND_MULTIPLE -> streamListExtra(i)
        else -> emptyList()
    }.ifEmpty {
        // Some apps only put the stream into ClipData.
        val clip = i.clipData ?: return@ifEmpty emptyList()
        (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri }
    }

    private fun sharedText(i: Intent): String? {
        val text = i.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
        val subject = i.getStringExtra(Intent.EXTRA_SUBJECT)
        return when {
            text.isNullOrBlank() -> subject
            subject.isNullOrBlank() || text.contains(subject) || Format.isUrl(text) -> text
            else -> "$subject\n$text"
        }
    }

    @Suppress("DEPRECATION")
    private fun streamExtra(i: Intent): Uri? =
        if (Build.VERSION.SDK_INT >= 33) i.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        else i.getParcelableExtra(Intent.EXTRA_STREAM)

    @Suppress("DEPRECATION")
    private fun streamListExtra(i: Intent): List<Uri> =
        (if (Build.VERSION.SDK_INT >= 33) i.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
        else i.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)).orEmpty()

    private fun toast(msg: String) = Toast.makeText(applicationContext, msg, Toast.LENGTH_SHORT).show()
}
