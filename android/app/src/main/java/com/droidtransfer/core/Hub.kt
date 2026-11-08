package com.droidtransfer.core

import android.content.*
import android.net.*
import android.net.wifi.WifiManager
import android.os.*
import org.json.JSONObject
import java.net.*
import java.util.UUID
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicLong

data class HubState(val myName: String,val pcs: List<PcInfo>,val transfers: List<TransferInfo>,val pairing: PairingStatus?,val texts: List<TextInfo>,val receiveInBackground: Boolean,val serviceRunning: Boolean,val wifiConnected: Boolean)
data class PcInfo(val id: String,val name: String,val ip: String?,val online: Boolean,val paired: Boolean)
enum class TransferStatus { RUNNING,DONE,FAILED,CANCELLED }
data class TransferInfo(val id: Long,val pcName: String,val outgoing: Boolean,val currentFile: String,val filesDone: Int,val filesTotal: Int,val bytesDone: Long,val bytesTotal: Long,val speedBps: Double,val status: TransferStatus,val error: String?)
enum class PairingPhase { CONNECTING,WAITING_FOR_PC,SUCCESS,FAILED }
data class PairingStatus(val pcId: String,val pcName: String,val phase: PairingPhase,val code: String?,val message: String?)
data class TextInfo(val pcName: String,val text: String,val incoming: Boolean,val atMillis: Long)

object Hub {
    internal lateinit var context: Context
    internal lateinit var prefs: SharedPreferences
    internal lateinit var identity: Identity
    private val main=Handler(Looper.getMainLooper())
    private val listeners=CopyOnWriteArraySet<(HubState)->Unit>()
    private val workers=Executors.newCachedThreadPool()
    private val timer=Executors.newSingleThreadScheduledExecutor()
    internal val ids=AtomicLong(System.currentTimeMillis())
    private val peers=linkedMapOf<String,Seen>()
    private val pairs=linkedMapOf<String,PairRecord>()
    private var scanning=false
    private var running=false
    private var udp: DatagramSocket?=null
    private var multicast: WifiManager.MulticastLock?=null
    private var pairingSocket: Socket?=null
    private var pairingGeneration=0L
    private var lastPublish=0L
    private data class Seen(val name: String,val ip: String,val port: Int,val at: Long)
    internal data class PairRecord(val name: String,val secretHex: String,val lastIp: String?)
    @Volatile var state=HubState("", emptyList(),emptyList(),null,emptyList(),false,false,false)
        private set

    @Synchronized fun init(context: Context) {
        if(this::context.isInitialized) return
        this.context=context.applicationContext
        prefs=this.context.getSharedPreferences("droidtransfer",Context.MODE_PRIVATE)
        val id=prefs.getString("id",null) ?: UUID.randomUUID().toString().also { prefs.edit().putString("id",it).apply() }
        identity=Identity(id,Build.MANUFACTURER+" "+Build.MODEL,"phone")
        runCatching { val root=JSONObject(prefs.getString("pairs","{}")!!); root.keys().forEach { key -> val p=root.getJSONObject(key); Protocol.unhex(p.getString("secretHex"),32); pairs[key]=PairRecord(p.getString("name"),p.getString("secretHex"),p.optString("lastIp").ifEmpty { null }) } }
        state=state.copy(myName=identity.name,receiveInBackground=prefs.getBoolean("receive_in_background",false))
        workers.execute { listenDiscovery() }
        timer.scheduleAtFixedRate({ runCatching { synchronized(this) { if(scanning) broadcast(); publish(true) } } },0,3,TimeUnit.SECONDS)
        publish(true)
        if(state.receiveInBackground) send(Intent(this.context,TransferService::class.java))
    }
    fun addListener(l: (HubState)->Unit) { listeners.add(l); if(Looper.myLooper()==Looper.getMainLooper()) l(state) else main.post { if(listeners.contains(l)) l(state) } }
    fun removeListener(l: (HubState)->Unit) { listeners.remove(l) }
    @Synchronized internal fun publish(force: Boolean=false) {
        val now=SystemClock.elapsedRealtime()
        if(!force && now-lastPublish<100) return
        lastPublish=now
        val cm=context.getSystemService(ConnectivityManager::class.java)
        val wifi=cm.getNetworkCapabilities(cm.activeNetwork)?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)==true
        val pcs=(pairs.keys+peers.keys).distinct().map { id -> val seen=peers[id]; val p=pairs[id]; PcInfo(id,seen?.name ?: p!!.name,seen?.ip ?: p?.lastIp,seen!=null && now-seen.at<15000,p!=null) }
        state=state.copy(pcs=pcs,serviceRunning=running,wifiConnected=wifi)
        val snapshot=state
        main.post { listeners.forEach { l -> runCatching { l(snapshot) } } }
    }
    @Synchronized fun setScanning(active: Boolean) { scanning=active; updateMulticast(); updateService(); if(active) workers.execute { broadcast() } }
    @Synchronized internal fun keepReceiving()=scanning || state.receiveInBackground
    private fun updateService() {
        if(keepReceiving()) send(Intent(context,TransferService::class.java))
        else if(running) runCatching { context.startService(Intent(context,TransferService::class.java).setAction(TransferService.IDLE)) }
    }
    @Synchronized internal fun service(active: Boolean) { running=active; updateMulticast(); publish(true); if(active) workers.execute { broadcast() } }
    private fun updateMulticast() {
        if(scanning || running) { if(multicast==null) multicast=context.getSystemService(WifiManager::class.java).createMulticastLock("DroidTransfer:discovery").apply { setReferenceCounted(false) }; if(multicast?.isHeld==false) multicast?.acquire() }
        else if(multicast?.isHeld==true) multicast?.release()
    }
    private fun packet(type: String)=Json.encode(mapOf("t" to type,"v" to 1,"id" to identity.id,"name" to identity.name,"kind" to "phone","port" to 47822)).toByteArray(Charsets.UTF_8)
    private fun listenDiscovery() {
        while(true) {
            try {
                val socket=DatagramSocket(null).apply { reuseAddress=true; broadcast=true; bind(InetSocketAddress(47820)) }; udp=socket
                socket.use { while(true) {
                    val p=DatagramPacket(ByteArray(1025),1025); socket.receive(p); if(p.length>1024) continue
                    runCatching { val m=Json.parse(p.data.copyOf(p.length).toString(Charsets.UTF_8)); val type=m["t"]; if(m["v"]!=1L || m["kind"]!="pc" || m["id"]==identity.id || (type!="here" && type!="disc")) return@runCatching
                        val id=m["id"] as String; val name=m["name"] as String; val port=(m["port"] as Number).toInt(); require(port in 1..65535)
                        synchronized(this) { peers[id]=Seen(name,p.address.hostAddress!!,port,SystemClock.elapsedRealtime()); publish(true) }
                        if(type=="disc") { val reply=packet("here"); socket.send(DatagramPacket(reply,reply.size,p.address,p.port)) }
                    }
                } }
            } catch(_: Exception) { udp=null; Thread.sleep(3000) }
        }
    }
    internal fun broadcast() {
        runCatching {
            val addresses=linkedSetOf("255.255.255.255")
            runCatching { NetworkInterface.getNetworkInterfaces()?.toList()?.forEach { network ->
                runCatching { if(network.isUp && !network.isLoopback) network.interfaceAddresses.forEach { link ->
                    if(link.address is Inet4Address) link.broadcast?.hostAddress?.let { addresses.add(it) }
                } }
            } }
            val cm=context.getSystemService(ConnectivityManager::class.java)
            cm.getLinkProperties(cm.activeNetwork)?.linkAddresses?.forEach { link ->
                if(link.address is Inet4Address && link.prefixLength in 1..30) {
                    val b=link.address.address; var ip=0; b.forEach { ip=(ip shl 8) or (it.toInt() and 255) }; val result=ip or (-1 ushr link.prefixLength)
                    addresses.add((3 downTo 0).joinToString(".") { ((result ushr (it*8)) and 255).toString() })
                }
            }
            val bytes=packet("disc"); if(bytes.size<=1024) addresses.forEach { address -> runCatching { udp?.send(DatagramPacket(bytes,bytes.size,InetAddress.getByName(address),47820)) } }
        }
    }
    @Synchronized internal fun record(id: String)=pairs[id]
    @Synchronized internal fun endpoint(id: String): Pair<String,Int> { val seen=peers[id]; val ip=seen?.ip ?: pairs[id]?.lastIp ?: error("PC address unknown; scan first"); return ip to (seen?.port ?: 47821) }
    @Synchronized internal fun remember(id: String,name: String,ip: String,secret: ByteArray?=null) {
        val existing=pairs[id]
        if(secret!=null || existing!=null) pairs[id]=PairRecord(name,secret?.let { Protocol.hex(it) } ?: existing!!.secretHex,ip)
        peers[id]=Seen(name,ip,peers[id]?.port ?: 47821,SystemClock.elapsedRealtime())
        persistPairs(); prefs.edit().putString("last_pc",id).apply(); publish(true)
    }
    private fun persistPairs() { val root=JSONObject(); pairs.forEach { (id,p) -> root.put(id,JSONObject().put("name",p.name).put("secretHex",p.secretHex).put("lastIp",p.lastIp ?: "")) }; prefs.edit().putString("pairs",root.toString()).apply() }
    @Synchronized fun pair(pcId: String) {
        pairingSocket?.close(); val generation=++pairingGeneration; val name=state.pcs.find { it.id==pcId }?.name ?: pcId
        state=state.copy(pairing=PairingStatus(pcId,name,PairingPhase.CONNECTING,null,null)); publish(true)
        workers.execute {
            try {
                val (ip,port)=endpoint(pcId)
                Socket().use { socket ->
                    synchronized(this) { if(generation!=pairingGeneration) return@execute; pairingSocket=socket }
                    configure(socket); socket.connect(InetSocketAddress(ip,port),10000); socket.soTimeout=65000
                    val result=Protocol.client(wire(socket),identity,pcId,null) { code -> synchronized(this) { if(generation==pairingGeneration) { state=state.copy(pairing=PairingStatus(pcId,name,PairingPhase.WAITING_FOR_PC,code,null)); publish(true) } } }
                    synchronized(this) { if(generation==pairingGeneration) { remember(pcId,result.first.name,ip,result.second ?: error("Missing pairing secret")); state=state.copy(pairing=state.pairing?.copy(phase=PairingPhase.SUCCESS)); publish(true) } }
                }
            } catch(e: Exception) { synchronized(this) { if(generation==pairingGeneration) { state=state.copy(pairing=PairingStatus(pcId,name,PairingPhase.FAILED,null,e.message)); publish(true) } } }
            finally { synchronized(this) { if(generation==pairingGeneration) pairingSocket=null } }
        }
    }
    @Synchronized fun dismissPairing() { pairingGeneration++; pairingSocket?.close(); pairingSocket=null; state=state.copy(pairing=null); publish(true) }
    @Synchronized fun unpair(pcId: String) { pairs.remove(pcId); persistPairs(); publish(true) }
    fun sendUris(pcId: String,uris: List<Uri>) { send(Intent(context,TransferService::class.java).setAction(TransferService.SEND).putExtra("pc",pcId).putParcelableArrayListExtra("uris",ArrayList(uris))) }
    fun sendTree(pcId: String,treeUri: Uri) { send(Intent(context,TransferService::class.java).setAction(TransferService.SEND).putExtra("pc",pcId).putExtra("tree",treeUri)) }
    private val pendingTexts=ConcurrentHashMap<Long,String>()
    internal fun storeText(text: String): Long=ids.incrementAndGet().also { pendingTexts[it]=text }
    internal fun takeText(id: Long)=pendingTexts.remove(id)
    fun sendText(pcId: String,text: String) {
        val id=storeText(text)
        if(!send(Intent(context,TransferService::class.java).setAction(TransferService.SEND).putExtra("pc",pcId).putExtra("textId",id))) takeText(id)
    }
    private fun send(intent: Intent): Boolean {
        return try { context.startForegroundService(intent); true } catch(e: RuntimeException) {
            // Includes ForegroundServiceStartNotAllowedException (API 31), security and state failures.
            if(intent.action==TransferService.SEND) synchronized(this) {
                val pc=intent.getStringExtra("pc") ?: ""
                transfer(TransferInfo(ids.incrementAndGet(),pairs[pc]?.name ?: pc,true,"",0,0,0,0,0.0,TransferStatus.FAILED,e.message),true)
            }
            publish(true); false
        }
    }
    fun cancel(transferId: Long) { TransferService.cancel(transferId) }
    @Synchronized fun clearFinished() { state=state.copy(transfers=state.transfers.filter { it.status==TransferStatus.RUNNING }); publish(true) }
    @Synchronized fun setReceiveInBackground(enabled: Boolean) { prefs.edit().putBoolean("receive_in_background",enabled).apply(); state=state.copy(receiveInBackground=enabled); publish(true); updateService() }
    @Synchronized fun defaultPcId(): String?=if(pairs.size==1) pairs.keys.first() else prefs.getString("last_pc",null)?.takeIf { pairs.containsKey(it) }
    @Synchronized internal fun transfer(info: TransferInfo,force: Boolean=false) { state=state.copy(transfers=state.transfers.filterNot { it.id==info.id }+info); publish(force) }
    @Synchronized internal fun text(name: String,text: String,incoming: Boolean) { state=state.copy(texts=(state.texts+TextInfo(name,text,incoming,System.currentTimeMillis())).takeLast(100)); publish(true) }
    internal fun configure(socket: Socket) { socket.sendBufferSize=4*1024*1024; socket.receiveBufferSize=4*1024*1024; socket.tcpNoDelay=true; socket.soTimeout=60000 }
    internal fun wire(socket: Socket)=Wire(socket.getInputStream().buffered(Protocol.BUFFER),socket.getOutputStream().buffered(Protocol.BUFFER))
}
