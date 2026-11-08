package com.droidtransfer.ui

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast
import com.droidtransfer.R
import com.droidtransfer.core.Hub
import com.droidtransfer.core.HubState
import com.droidtransfer.core.PairingPhase
import com.droidtransfer.core.PairingStatus
import com.droidtransfer.core.PcInfo
import com.droidtransfer.core.TextInfo
import com.droidtransfer.core.TransferInfo
import com.droidtransfer.core.TransferStatus

class MainActivity : Activity() {

    private lateinit var myName: TextView
    private lateinit var wifiWarning: TextView
    private lateinit var receiveSwitch: Switch
    private lateinit var receiveHint: TextView
    private lateinit var pcList: LinearLayout
    private lateinit var pcEmpty: TextView
    private lateinit var sendTitle: TextView
    private lateinit var sendCard: LinearLayout
    private lateinit var sendButtons: List<View>
    private lateinit var textInput: EditText
    private lateinit var transferList: LinearLayout
    private lateinit var transferEmpty: TextView
    private lateinit var btnClear: TextView
    private lateinit var textsTitle: TextView
    private lateinit var textList: LinearLayout

    private val inflater by lazy { LayoutInflater.from(this) }
    private val transferViews = HashMap<Long, View>()
    private var renderedTexts: List<TextInfo> = emptyList()

    private var selectedPcId: String? = null
    private var targetPc: PcInfo? = null
    private var updatingSwitch = false

    private var pairDialog: AlertDialog? = null
    private var pairDialogView: View? = null
    private var pairDialogKey: String? = null

    private val listener: (HubState) -> Unit = { render(it) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Hub.init(applicationContext)
        setContentView(R.layout.activity_main)
        selectedPcId = savedInstanceState?.getString(KEY_SELECTED)

        myName = findViewById(R.id.myName)
        wifiWarning = findViewById(R.id.wifiWarning)
        receiveSwitch = findViewById(R.id.receiveSwitch)
        receiveHint = findViewById(R.id.receiveHint)
        pcList = findViewById(R.id.pcList)
        pcEmpty = findViewById(R.id.pcEmpty)
        sendTitle = findViewById(R.id.sendTitle)
        sendCard = findViewById(R.id.sendCard)
        textInput = findViewById(R.id.textInput)
        transferList = findViewById(R.id.transferList)
        transferEmpty = findViewById(R.id.transferEmpty)
        btnClear = findViewById(R.id.btnClear)
        textsTitle = findViewById(R.id.textsTitle)
        textList = findViewById(R.id.textList)

        val btnFiles = findViewById<Button>(R.id.btnFiles)
        val btnFolder = findViewById<Button>(R.id.btnFolder)
        val btnClipboard = findViewById<Button>(R.id.btnClipboard)
        val btnSendText = findViewById<Button>(R.id.btnSendText)
        sendButtons = listOf(btnFiles, btnFolder, btnClipboard, btnSendText, textInput)

        btnFiles.setOnClickListener {
            val i = Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType("*/*")
                .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
            startActivityForResult(i, REQ_FILES)
        }
        btnFolder.setOnClickListener {
            startActivityForResult(Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), REQ_FOLDER)
        }
        btnClipboard.setOnClickListener {
            val cm = getSystemService(ClipboardManager::class.java)
            val text = cm?.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this)?.toString()
            if (text.isNullOrBlank()) toast("Clipboard is empty") else sendText(text)
        }
        btnSendText.setOnClickListener {
            val text = textInput.text.toString()
            if (text.isNotBlank()) {
                sendText(text)
                textInput.setText("")
            }
        }
        btnClear.setOnClickListener { Hub.clearFinished() }
        receiveSwitch.setOnCheckedChangeListener { _, checked ->
            if (!updatingSwitch) Hub.setReceiveInBackground(checked)
        }

        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), REQ_NOTIF)
        }
    }

    override fun onStart() {
        super.onStart()
        Hub.addListener(listener)
        Hub.setScanning(true)
    }

    override fun onStop() {
        Hub.setScanning(false)
        Hub.removeListener(listener)
        super.onStop()
    }

    override fun onDestroy() {
        pairDialog?.dismiss()
        super.onDestroy()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putString(KEY_SELECTED, selectedPcId)
    }

    @Deprecated("Framework Activity API")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (resultCode != RESULT_OK || data == null) return
        val pc = targetPc ?: return toast("Pair with a PC first")
        when (requestCode) {
            REQ_FILES -> {
                val uris = ArrayList<Uri>()
                data.clipData?.let { clip -> for (i in 0 until clip.itemCount) uris += clip.getItemAt(i).uri }
                if (uris.isEmpty()) data.data?.let { uris += it }
                if (uris.isEmpty()) return
                uris.forEach { keepReadAccess(it) }
                Hub.sendUris(pc.id, uris)
                toast("Sending ${uris.size} file(s) to ${pc.name}")
            }
            REQ_FOLDER -> {
                val tree = data.data ?: return
                keepReadAccess(tree, persistable = true)
                Hub.sendTree(pc.id, tree)
                toast("Sending folder to ${pc.name}")
            }
        }
    }

    private fun sendText(text: String) {
        val pc = targetPc ?: return toast("Pair with a PC first")
        Hub.sendText(pc.id, text)
        toast("Text sent to ${pc.name}")
    }

    // ------------------------------------------------------------------ rendering

    private fun render(s: HubState) {
        myName.text = s.myName
        wifiWarning.visibility = if (s.wifiConnected) View.GONE else View.VISIBLE

        updatingSwitch = true
        receiveSwitch.isChecked = s.receiveInBackground
        updatingSwitch = false
        receiveHint.text = when {
            s.receiveInBackground && s.serviceRunning -> "On: your PC can send files any time"
            s.receiveInBackground -> "Starting…"
            else -> "Off: your PC can send only while this app is open"
        }

        renderPcs(s)
        renderPairing(s.pairing)
        renderTransfers(s.transfers)
        renderTexts(s.texts)
    }

    private fun resolveTarget(s: HubState): PcInfo? {
        val paired = s.pcs.filter { it.paired }
        selectedPcId?.let { id -> paired.firstOrNull { it.id == id }?.let { return it } }
        val def = Hub.defaultPcId()
        return paired.firstOrNull { it.id == def } ?: paired.singleOrNull()
    }

    private fun renderPcs(s: HubState) {
        targetPc = resolveTarget(s)
        val anyPaired = s.pcs.any { it.paired }
        pcList.removeAllViews()
        pcEmpty.visibility = if (s.pcs.isEmpty()) View.VISIBLE else View.GONE

        for (pc in s.pcs) {
            val v = inflater.inflate(R.layout.item_pc, pcList, false)
            val selected = pc.paired && pc.id == targetPc?.id
            v.setBackgroundResource(if (selected) R.drawable.bg_card_selected else R.drawable.bg_card)
            v.findViewById<View>(R.id.dot).backgroundTintList =
                ColorStateList.valueOf(getColor(if (pc.online) R.color.online else R.color.offline))
            v.findViewById<TextView>(R.id.name).text = pc.name
            val where = pc.ip ?: "unknown address"
            v.findViewById<TextView>(R.id.detail).text = when {
                pc.paired && pc.online -> "$where · paired"
                pc.paired -> "$where · offline"
                else -> "$where · not paired"
            }
            val action = v.findViewById<Button>(R.id.action)
            if (pc.paired) {
                action.text = if (selected) "Selected" else "Use"
                action.isEnabled = !selected
                action.setOnClickListener { selectedPcId = pc.id; render(Hub.state) }
                v.setOnClickListener { selectedPcId = pc.id; render(Hub.state) }
                v.setOnLongClickListener { confirmUnpair(pc); true }
            } else {
                action.text = "Pair"
                action.isEnabled = pc.online && s.pairing?.phase.let { it == null || it == PairingPhase.FAILED || it == PairingPhase.SUCCESS }
                action.setOnClickListener { Hub.pair(pc.id) }
            }
            pcList.addView(v)
        }

        val pc = targetPc
        sendTitle.text = if (pc != null) "Send to ${pc.name}" else "Send"
        val enabled = pc != null
        sendButtons.forEach { it.isEnabled = enabled }
        sendCard.alpha = if (enabled) 1f else 0.5f
        if (!anyPaired && s.pcs.isNotEmpty()) sendTitle.text = "Send (pair a PC first)"
    }

    private fun confirmUnpair(pc: PcInfo) {
        AlertDialog.Builder(this)
            .setTitle("Unpair ${pc.name}?")
            .setMessage("You will need to accept a new pairing on the PC to transfer again.")
            .setPositiveButton("Unpair") { _, _ ->
                if (selectedPcId == pc.id) selectedPcId = null
                Hub.unpair(pc.id)
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun renderPairing(p: PairingStatus?) {
        if (p == null) {
            pairDialog?.dismiss()
            pairDialog = null
            pairDialogKey = null
            return
        }
        val key = p.pcId + ":" + p.phase
        if (pairDialog == null) {
            val view = inflater.inflate(R.layout.dialog_pair, null)
            pairDialogView = view
            pairDialog = AlertDialog.Builder(this)
                .setTitle("Pair with ${p.pcName}")
                .setView(view)
                .setNegativeButton("Cancel") { _, _ -> Hub.dismissPairing() }
                .setOnCancelListener { Hub.dismissPairing() }
                .show()
        }
        if (key == pairDialogKey) return
        pairDialogKey = key

        val view = pairDialogView ?: return
        val msg = view.findViewById<TextView>(R.id.pairMessage)
        val code = view.findViewById<TextView>(R.id.pairCode)
        val spinner = view.findViewById<ProgressBar>(R.id.pairSpinner)
        val button = pairDialog?.getButton(AlertDialog.BUTTON_NEGATIVE)

        code.visibility = if (p.code != null && p.phase != PairingPhase.FAILED) View.VISIBLE else View.GONE
        code.text = p.code?.let { Format.code(it) }
        when (p.phase) {
            PairingPhase.CONNECTING -> {
                msg.text = "Connecting to ${p.pcName}…"
                spinner.visibility = View.VISIBLE
                button?.text = "Cancel"
            }
            PairingPhase.WAITING_FOR_PC -> {
                msg.text = "Click Accept on your PC.\nMake sure it shows this code:"
                spinner.visibility = View.VISIBLE
                button?.text = "Cancel"
            }
            PairingPhase.SUCCESS -> {
                msg.text = "Paired with ${p.pcName}."
                spinner.visibility = View.GONE
                button?.text = "Done"
                selectedPcId = p.pcId
                msg.postDelayed({ if (Hub.state.pairing?.phase == PairingPhase.SUCCESS) Hub.dismissPairing() }, 1200)
            }
            PairingPhase.FAILED -> {
                msg.text = "Pairing failed" + (p.message?.let { ": $it" } ?: "")
                spinner.visibility = View.GONE
                button?.text = "Close"
            }
        }
    }

    private fun renderTransfers(list: List<TransferInfo>) {
        transferEmpty.visibility = if (list.isEmpty()) View.VISIBLE else View.GONE
        btnClear.visibility = if (list.any { it.status != TransferStatus.RUNNING }) View.VISIBLE else View.GONE

        val ids = list.map { it.id }.toSet()
        transferViews.keys.filter { it !in ids }.forEach { transferList.removeView(transferViews.remove(it)) }
        list.forEachIndexed { index, t ->
            val v = transferViews.getOrPut(t.id) { inflater.inflate(R.layout.item_transfer, transferList, false) }
            if (transferList.indexOfChild(v) != index) {
                (v.parent as? ViewGroup)?.removeView(v)
                transferList.addView(v, index)
            }
            bindTransfer(v, t)
        }
    }

    private fun bindTransfer(v: View, t: TransferInfo) {
        v.findViewById<TextView>(R.id.title).text = (if (t.outgoing) "↑ To " else "↓ From ") + t.pcName
        val state = v.findViewById<TextView>(R.id.state)
        val cancel = v.findViewById<TextView>(R.id.cancel)
        val progress = v.findViewById<ProgressBar>(R.id.progress)
        val file = v.findViewById<TextView>(R.id.file)
        val info = v.findViewById<TextView>(R.id.info)

        val running = t.status == TransferStatus.RUNNING
        cancel.visibility = if (running) View.VISIBLE else View.GONE
        cancel.setOnClickListener { Hub.cancel(t.id) }
        progress.visibility = if (running) View.VISIBLE else View.GONE
        state.visibility = if (running) View.GONE else View.VISIBLE
        when (t.status) {
            TransferStatus.DONE -> { state.text = "✓ Done"; state.setTextColor(getColor(R.color.online)) }
            TransferStatus.FAILED -> { state.text = "Failed"; state.setTextColor(getColor(R.color.danger)) }
            TransferStatus.CANCELLED -> { state.text = "Cancelled"; state.setTextColor(getColor(R.color.offline)) }
            TransferStatus.RUNNING -> {}
        }

        file.text = t.currentFile
        file.visibility = if (t.currentFile.isNotEmpty() && running) View.VISIBLE else View.GONE

        val frac = when {
            t.bytesTotal > 0 -> t.bytesDone.toDouble() / t.bytesTotal
            t.filesTotal > 0 -> t.filesDone.toDouble() / t.filesTotal
            else -> 0.0
        }
        progress.progress = (frac.coerceIn(0.0, 1.0) * 1000).toInt()

        val sb = StringBuilder("${t.filesDone}/${t.filesTotal} files · ${Format.bytes(t.bytesDone)}")
        if (t.bytesTotal > 0) sb.append(" of ${Format.bytes(t.bytesTotal)}")
        if (running && t.speedBps > 0) {
            sb.append(" · ${Format.bytes(t.speedBps.toLong())}/s")
            if (t.bytesTotal > t.bytesDone) sb.append(" · ${Format.duration(((t.bytesTotal - t.bytesDone) / t.speedBps).toLong())} left")
        }
        if (t.status == TransferStatus.FAILED && t.error != null) sb.append("\n").append(t.error)
        info.text = sb
    }

    private fun renderTexts(texts: List<TextInfo>) {
        textsTitle.visibility = if (texts.isEmpty()) View.GONE else View.VISIBLE
        if (texts == renderedTexts) return
        renderedTexts = texts
        textList.removeAllViews()
        for (t in texts.take(20)) {
            val v = inflater.inflate(R.layout.item_text, textList, false)
            v.findViewById<TextView>(R.id.meta).text =
                (if (t.incoming) "From ${t.pcName}" else "To ${t.pcName}") + " · " + Format.ago(t.atMillis)
            v.findViewById<TextView>(R.id.body).text = t.text
            v.findViewById<TextView>(R.id.copy).setOnClickListener {
                getSystemService(ClipboardManager::class.java)?.setPrimaryClip(ClipData.newPlainText("DroidTransfer", t.text))
                if (Build.VERSION.SDK_INT < 33) toast("Copied")
            }
            val open = v.findViewById<TextView>(R.id.open)
            if (Format.isUrl(t.text)) {
                open.visibility = View.VISIBLE
                open.setOnClickListener {
                    try {
                        startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(t.text.trim())))
                    } catch (e: Exception) {
                        toast("No app can open this link")
                    }
                }
            }
            textList.addView(v)
        }
    }

    private fun toast(msg: String) = Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()

    companion object {
        private const val REQ_FILES = 1
        private const val REQ_FOLDER = 2
        private const val REQ_NOTIF = 3
        private const val KEY_SELECTED = "selected_pc"
    }
}
