package com.droidtransfer.core

import android.app.*
import android.content.*
import android.content.pm.ServiceInfo
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.*
import com.droidtransfer.R
import java.io.IOException
import java.net.*
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicBoolean

class TransferService: Service() {
    companion object {
        const val SEND="com.droidtransfer.SEND"
        const val IDLE="com.droidtransfer.IDLE"
        private const val CANCEL="com.droidtransfer.CANCEL"
        private const val COPY="com.droidtransfer.COPY"
        private const val CHANNEL="transfers"
        private const val FOREGROUND_ID=1
        private val jobs=ConcurrentHashMap<Long,Job>()
        internal fun cancel(id: Long) { jobs[id]?.let { it.cancelled.set(true); runCatching { it.socket?.close() }; it.source.cancel() } }
        private class Job { @Volatile var socket: Socket?=null; val cancelled=AtomicBoolean(false); val source=TransferCancellation() }
        private val notificationTexts=object: LinkedHashMap<Long,String>(100,0.75f,true) {
            override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Long,String>?)=size>100
        }
    }
    private val executor=Executors.newCachedThreadPool()
    private val handshakes=Semaphore(4)
    private val pendingSockets=ConcurrentHashMap.newKeySet<Socket>()
    private val deadlines=ScheduledThreadPoolExecutor(1).apply { removeOnCancelPolicy=true }
    private val admission=ServiceAdmission()
    private var lockUsers=0
    private val main=Handler(Looper.getMainLooper())
    @Volatile private var alive=true
    private var foregroundReady=false
    private var server: ServerSocket?=null
    private lateinit var storage: Storage
    private var wifiLock: WifiManager.WifiLock?=null
    private var wakeLock: PowerManager.WakeLock?=null
    private var lastNotification=0L
    override fun onBind(intent: Intent?)=null
    override fun onCreate() {
        super.onCreate(); Hub.init(this); storage=Storage(this)
        getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel(CHANNEL,"File transfers",NotificationManager.IMPORTANCE_LOW))
        try {
            startForeground(FOREGROUND_ID,notification("Ready to receive files"),ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            foregroundReady=true
        } catch(_: RuntimeException) { alive=false; Hub.service(false); stopSelf(); return }
        executor.execute {
            try {
                val listener=ServerSocket().apply { reuseAddress=true; bind(InetSocketAddress(47822)) }; server=listener
                if(!alive) { listener.close(); return@execute }
                main.post { if(alive && !listener.isClosed) Hub.service(true) }
                listener.use { while(alive) {
                    val socket=it.accept()
                    val acceptedAt=SystemClock.elapsedRealtime()
                    if(!handshakes.tryAcquire()) { socket.close(); continue }
                    if(!admission.admit()) { socket.close(); handshakes.release(); continue }
                    pendingSockets.add(socket)
                    try {
                        if(!alive) throw IOException("Service stopped")
                        val deadline=HandshakeDeadline(acceptedAt,10000)
                        val timeout=deadlines.schedule({ if(deadline.expire(SystemClock.elapsedRealtime())) runCatching { socket.close() } },10000,TimeUnit.MILLISECONDS)
                        try { executor.execute { receive(socket,deadline,timeout) } } catch(e: Exception) { timeout.cancel(false); throw e }
                    } catch(e: Exception) { pendingSockets.remove(socket); runCatching { socket.close() }; handshakes.release(); end(false); if(alive) throw e }
                } }
            } catch(e: Exception) { if(alive) { Hub.service(false); notify(2,notification("Cannot receive: "+e.message)) } }
        }
    }
    override fun onStartCommand(intent: Intent?,flags: Int,startId: Int): Int {
        if(!foregroundReady) { intent?.getLongExtra("textId",-1)?.let { Hub.takeText(it) }; stopSelf(); return START_NOT_STICKY }
        when(intent?.action) {
            SEND -> {
                if(admission.admit()) {
                    try { executor.execute { outgoing(intent) } }
                    catch(e: RejectedExecutionException) { Hub.takeText(intent.getLongExtra("textId",-1)); end(false) }
                } else Hub.takeText(intent.getLongExtra("textId",-1))
            }
            CANCEL -> { cancel(intent.getLongExtra("id",-1)); idle() }
            COPY -> { synchronized(notificationTexts) { notificationTexts[intent.getLongExtra("textId",-1)] }?.let { copyText(it) }; idle() }
            else -> idle()
        }
        return if(Hub.state.receiveInBackground) START_STICKY else START_NOT_STICKY
    }
    @Synchronized private fun acquireTransferLocks() {
        check(alive) { "Service stopped" }
        if(lockUsers++==0) {
            val wm=getSystemService(WifiManager::class.java)
            @Suppress("DEPRECATION")
            val mode=if(Build.VERSION.SDK_INT>=34) WifiManager.WIFI_MODE_FULL_LOW_LATENCY else WifiManager.WIFI_MODE_FULL_HIGH_PERF
            runCatching { wifiLock=wm.createWifiLock(mode,"DroidTransfer:transfer").apply { setReferenceCounted(false); acquire() } }
            runCatching { wakeLock=getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK,"DroidTransfer:transfer").apply { setReferenceCounted(false); acquire() } }
        }
    }
    @Synchronized private fun end(hasLocks: Boolean) {
        if(hasLocks && --lockUsers==0) releaseLocks()
        if(admission.finish()) { notify(FOREGROUND_ID,notification("Ready to receive files")); idle() }
    }
    @Synchronized private fun releaseLocks() { if(wifiLock?.isHeld==true) wifiLock?.release(); if(wakeLock?.isHeld==true) wakeLock?.release(); wifiLock=null; wakeLock=null }
    private fun idle() { main.post { if(admission.tryShutdown { Hub.keepReceiving() }) stopSelf() } }
    private inner class Progress(val id: Long,val name: String,val outgoing: Boolean) {
        private val started=SystemClock.elapsedRealtime()
        var info=TransferInfo(id,name,outgoing,"",0,0,0,0,0.0,TransferStatus.RUNNING,null)
        init { Hub.transfer(info,true); showProgress(info) }
        fun totals(count: Int,bytes: Long) { info=info.copy(filesTotal=count,bytesTotal=bytes); Hub.transfer(info,true) }
        fun update(path: String,done: Int,bytes: Long) {
            if(jobs[id]?.cancelled?.get()==true) throw IOException("Cancelled")
            val elapsed=(SystemClock.elapsedRealtime()-started).coerceAtLeast(1)
            info=info.copy(currentFile=path,filesDone=done,bytesDone=bytes,speedBps=bytes*1000.0/elapsed)
            Hub.transfer(info); showProgress(info)
        }
        fun finish(error: Exception?=null) {
            val cancelled=jobs[id]?.cancelled?.get()==true
            info=info.copy(status=if(cancelled) TransferStatus.CANCELLED else if(error!=null) TransferStatus.FAILED else TransferStatus.DONE,error=if(cancelled) null else error?.message)
            Hub.transfer(info,true)
            if(error==null && !cancelled && info.filesTotal>0) {
                val title=(if(outgoing) "Sent " else "Received ")+info.filesTotal+" files "+(if(outgoing) "to " else "from ")+name
                val builder=base(title)
                if(!outgoing) builder.setContentIntent(PendingIntent.getActivity(this@TransferService,id.toInt(),Intent(DownloadManager.ACTION_VIEW_DOWNLOADS),PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
                notify(id.toInt(),builder.build())
            } else if(error!=null && !cancelled) notify(id.toInt(),notification("Transfer failed: "+error.message))
        }
    }
    private fun outgoing(intent: Intent) {
        val id=Hub.ids.incrementAndGet(); val job=Job(); jobs[id]=job
        var hasLocks=false
        val pc=intent.getStringExtra("pc") ?: ""; val name=Hub.state.pcs.find { it.id==pc }?.name ?: pc; val progress=Progress(id,name,true)
        try {
            val record=Hub.record(pc) ?: error("PC is not paired")
            @Suppress("DEPRECATION") val uris=intent.getParcelableArrayListExtra<Uri>("uris")
            @Suppress("DEPRECATION") val tree=intent.getParcelableExtra<Uri>("tree")
            val text=if(intent.hasExtra("textId")) Hub.takeText(intent.getLongExtra("textId",-1)) ?: error("Text no longer available") else null
            val sources=if(text!=null) emptyList() else if(tree!=null) storage.tree(tree,job.source) else (uris ?: emptyList()).map { job.source.check(); storage.source(it,cancellation=job.source) }
            progress.totals(sources.size,sources.sumOf { it.size.coerceAtLeast(0) })
            Socket().use { socket ->
                job.socket=socket; if(job.cancelled.get()) throw IOException("Cancelled")
                Hub.configure(socket); val (ip,port)=Hub.endpoint(pc); socket.connect(InetSocketAddress(ip,port),10000)
                val w=Hub.wire(socket); Protocol.client(w,Hub.identity,pc,Protocol.unhex(record.secretHex,32)); Hub.remember(pc,name,ip)
                job.source.check(); acquireTransferLocks(); hasLocks=true
                if(text!=null) { Protocol.sendText(w,text); Hub.text(name,text,false) } else Protocol.sendBatch(w,sources,progress::update)
            }
            progress.finish()
        } catch(e: Exception) { progress.finish(e) }
        finally { Hub.takeText(intent.getLongExtra("textId",-1)); job.source.close(); jobs.remove(id); end(hasLocks) }
    }
    private fun receive(socket: Socket,deadline: HandshakeDeadline,timeout: ScheduledFuture<*>) {
        val id=Hub.ids.incrementAndGet(); val job=Job(); job.socket=socket; jobs[id]=job; var progress: Progress?=null
        var hasLocks=false
        var pending=true
        try { socket.use {
            if(!alive) throw IOException("Service stopped")
            Hub.configure(it); val w=Hub.wire(it)
            val peer=try { Protocol.server(w,Hub.identity,{ peerId -> Hub.record(peerId)?.let { p -> Protocol.unhex(p.secretHex,32) } }) } catch(e: Exception) { w.error(e.message ?: "auth failed"); throw e }
            requireNotNull(peer)
            if(!deadline.authenticate(SystemClock.elapsedRealtime())) throw SocketTimeoutException("Authentication timed out")
            timeout.cancel(false); pendingSockets.remove(socket); handshakes.release(); pending=false
            acquireTransferLocks(); hasLocks=true
            Hub.remember(peer.id,peer.name,it.inetAddress.hostAddress!!)
            val p=Progress(id,peer.name,false); progress=p
            Protocol.receive(w,{ path,size -> storage.sink(path,size,job.source) },{ text -> Hub.text(peer.name,text,true); copyText(text); textNotification(peer.name,text) },p::totals,p::update); p.finish()
        } } catch(e: Exception) { progress?.finish(e) }
        finally { timeout.cancel(false); pendingSockets.remove(socket); if(pending) handshakes.release(); job.source.close(); jobs.remove(id); end(hasLocks) }
    }
    private fun copyText(text: String) { runCatching { getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("DroidTransfer",text)) } }
    private fun textNotification(name: String,text: String) {
        val textId=Hub.ids.incrementAndGet(); val id=textId.toInt()
        synchronized(notificationTexts) { notificationTexts[textId]=text }
        val action=Intent(this,TransferService::class.java).setAction(COPY).putExtra("textId",textId)
        val pi=PendingIntent.getForegroundService(this,id,action,PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val body=text.take(1000)
        notify(id,base("Text from "+name).setContentText(body).setStyle(Notification.BigTextStyle().bigText(body)).addAction(Notification.Action.Builder(null,"Copy",pi).build()).build())
    }
    private fun base(title: String)=Notification.Builder(this,CHANNEL).setSmallIcon(R.drawable.ic_stat_transfer).setContentTitle(title).setAutoCancel(true)
    private fun notification(title: String)=base(title).build()
    private fun notify(id: Int,n: Notification) { runCatching { getSystemService(NotificationManager::class.java).notify(id,n) } }
    @Synchronized private fun showProgress(info: TransferInfo) {
        val now=SystemClock.elapsedRealtime(); if(now-lastNotification<100) return; lastNotification=now
        val percent=if(info.bytesTotal>0) ((info.bytesDone.toDouble()/info.bytesTotal)*100).toInt().coerceIn(0,100) else 0
        val action=Intent(this,TransferService::class.java).setAction(CANCEL).putExtra("id",info.id)
        val pi=PendingIntent.getService(this,info.id.toInt(),action,PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        notify(FOREGROUND_ID,base((if(info.outgoing) "Sending to " else "Receiving from ")+info.pcName).setContentText(info.currentFile+" · "+percent+"% · "+"%.1f".format(info.speedBps/1048576)+" MiB/s").setProgress(100,percent,info.bytesTotal<=0).setOngoing(true).addAction(Notification.Action.Builder(null,"Cancel",pi).build()).build())
    }
    override fun onDestroy() {
        admission.shutdown(); alive=false; runCatching { server?.close() }; pendingSockets.forEach { runCatching { it.close() } }; jobs.keys.forEach { cancel(it) }; deadlines.shutdownNow(); executor.shutdown(); releaseLocks(); Hub.service(false); super.onDestroy()
    }
}

class BootReceiver: BroadcastReceiver() {
    override fun onReceive(context: Context,intent: Intent) {
        if(intent.action==Intent.ACTION_BOOT_COMPLETED && context.getSharedPreferences("droidtransfer",Context.MODE_PRIVATE).getBoolean("receive_in_background",false)) {
            runCatching { context.startForegroundService(Intent(context,TransferService::class.java)) }
        }
    }
}
