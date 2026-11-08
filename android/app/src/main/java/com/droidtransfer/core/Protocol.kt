package com.droidtransfer.core

import java.io.*
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** Small JVM-only JSON codec; no Android or runtime dependencies. */
object Json {
    fun encode(v: Any?): String = when(v) {
        null -> "null"
        is String -> buildString { append('"'); v.forEach { c -> when(c) { '"' -> append("\\\""); '\\' -> append("\\\\"); else -> if(c < ' ') append("\\u%04x".format(c.code)) else append(c) } }; append('"') }
        is Map<*,*> -> v.entries.joinToString(",","{","}") { encode(it.key.toString())+":"+encode(it.value) }
        is Iterable<*> -> v.joinToString(",","[","]") { encode(it) }
        is Number, is Boolean -> v.toString()
        else -> error("Unsupported JSON")
    }
    fun parse(text: String): Map<String,Any?> {
        class Parser {
            var i=0
            fun ws() { while(i<text.length && text[i].isWhitespace()) i++ }
            fun take(c: Char) { ws(); require(i<text.length && text[i++]==c) { "Invalid JSON" } }
            fun string(): String { take('"'); return buildString { while(true) { require(i<text.length); val c=text[i++]; if(c=='"') break; if(c=='\\') { require(i<text.length); append(when(val e=text[i++]) { '"','\\','/' -> e; 'b' -> '\b'; 'f' -> '\u000c'; 'n' -> '\n'; 'r' -> '\r'; 't' -> '\t'; 'u' -> { require(i+4<=text.length); text.substring(i,i+4).toInt(16).toChar().also { i+=4 } }; else -> error("Invalid escape") }) } else { require(c>=' '); append(c) } } } }
            fun value(depth: Int=0): Any? { require(depth<64); ws(); require(i<text.length); return when(text[i]) {
                '"' -> string()
                '{' -> { i++; val m=linkedMapOf<String,Any?>(); ws(); if(i<text.length && text[i]=='}') i++ else while(true) { val k=string(); take(':'); m[k]=value(depth+1); ws(); if(i<text.length && text[i]=='}') { i++; break }; take(',') }; m }
                '[' -> { i++; val a=mutableListOf<Any?>(); ws(); if(i<text.length && text[i]==']') i++ else while(true) { a.add(value(depth+1)); ws(); if(i<text.length && text[i]==']') { i++; break }; take(',') }; a }
                else -> { val start=i; while(i<text.length && text[i] !in ",}] \n\r\t") i++; when(val s=text.substring(start,i)) { "true" -> true; "false" -> false; "null" -> null; else -> s.toLongOrNull() ?: s.toDoubleOrNull() ?: error("Invalid JSON value") } }
            } }
        }
        val p=Parser(); val v=p.value(); p.ws(); require(p.i==text.length && v is Map<*,*>); @Suppress("UNCHECKED_CAST") return v as Map<String,Any?>
    }
}
data class Frame(val type: Int,val payload: ByteArray) { fun json()=Json.parse(payload.toString(Charsets.UTF_8)) }
class Wire(input: InputStream,output: OutputStream) {
    private val input=DataInputStream(input)
    private val output=DataOutputStream(output)
    fun read(): Frame { val t=input.readUnsignedByte(); val n=input.readInt(); require(n in 0..16*1024*1024) { "Frame too large" }; return Frame(t,ByteArray(n).also { input.readFully(it) }) }
    fun expect(type: Int): Map<String,Any?> { val f=read(); if(f.type==5) throw IOException(f.json()["msg"].toString()); require(f.type==type) { "Unexpected frame" }; return f.json() }
    fun frame(type: Int,bytes: ByteArray,length: Int=bytes.size) { require(length in 0..16*1024*1024); output.writeByte(type); output.writeInt(length); output.write(bytes,0,length); output.flush() }
    fun json(type: Int,vararg fields: Pair<String,Any?>)=frame(type,Json.encode(linkedMapOf(*fields)).toByteArray(Charsets.UTF_8))
    fun error(message: String) { runCatching { json(5,"msg" to message) } }
}
data class Identity(val id: String,val name: String,val kind: String)
data class PeerHello(val identity: Identity,val nonce: ByteArray,val paired: Boolean)
object Protocol {
    const val BUFFER=1024*1024
    fun hex(b: ByteArray)=b.joinToString("") { "%02x".format(it.toInt() and 255) }
    fun unhex(s: String,size: Int): ByteArray { require(s.length==size*2 && s.all { it in '0'..'9' || it in 'a'..'f' }); return ByteArray(size) { s.substring(it*2,it*2+2).toInt(16).toByte() } }
    fun nonce()=ByteArray(16).also { SecureRandom().nextBytes(it) }
    fun mac(key: ByteArray,server: Boolean,c: ByteArray,s: ByteArray): ByteArray=Mac.getInstance("HmacSHA256").run { init(SecretKeySpec(key,"HmacSHA256")); update((if(server) "DT1-S" else "DT1-C").toByteArray(Charsets.US_ASCII)); update(c); doFinal(s) }
    fun code(c: ByteArray,s: ByteArray): String { val h=MessageDigest.getInstance("SHA-256").digest(c+s); var n=0L; repeat(4) { n=(n shl 8) or (h[it].toLong() and 255) }; return (n%1000000).toString().padStart(6,'0') }
    private fun hello(m: Map<String,Any?>): PeerHello { require((m["v"] as? Number)?.toInt()==1); val kind=m["kind"] as String; require(kind=="pc" || kind=="phone"); return PeerHello(Identity(m["id"] as String,m["name"] as String,kind),unhex(m["nonce"] as String,16),m["paired"]==true) }
    fun client(w: Wire,me: Identity,expectedId: String,key: ByteArray?,pairing: ((String)->Unit)?=null): Pair<Identity,ByteArray?> {
        val c=nonce(); w.json(1,"v" to 1,"id" to me.id,"name" to me.name,"kind" to me.kind,"nonce" to hex(c)); val h=hello(w.expect(2)); require(h.identity.id==expectedId && h.identity.kind!=me.kind) { "Unexpected peer" }
        if(h.paired && key!=null) { w.json(3,"mac" to hex(mac(key,false,c,h.nonce))); require(MessageDigest.isEqual(mac(key,true,c,h.nonce),unhex(w.expect(4)["mac"] as String,32))) { "auth failed" }; return h.identity to null }
        require(pairing!=null && me.kind=="phone" && h.identity.kind=="pc") { "Not paired" }; w.json(6); pairing(code(c,h.nonce)); return h.identity to unhex(w.expect(7)["secret"] as String,32)
    }
    /** PC pairing hook permits complete JVM interoperability tests. Never supplied on Android. */
    fun server(w: Wire,me: Identity,lookup: (String)->ByteArray?,pair: ((Identity,String)->ByteArray?)?=null): Identity? {
        val h=hello(w.expect(1)); require(h.identity.kind!=me.kind); val s=nonce(); val key=lookup(h.identity.id); w.json(2,"v" to 1,"id" to me.id,"name" to me.name,"kind" to me.kind,"nonce" to hex(s),"paired" to (key!=null)); val f=w.read()
        if(f.type==6 && me.kind=="pc" && pair!=null) { val secret=pair(h.identity,code(h.nonce,s)); if(secret==null) { w.error("rejected"); return null }; require(secret.size==32); w.json(7,"secret" to hex(secret)); return null }
        if(f.type!=3 || key==null || !MessageDigest.isEqual(mac(key,false,h.nonce,s),unhex(f.json()["mac"] as String,32))) { w.error("auth failed"); throw IOException("auth failed") }; w.json(4,"mac" to hex(mac(key,true,h.nonce,s))); return h.identity
    }
    fun sanitize(path: String): String { val parts=path.split('/','\\').filter { it.isNotEmpty() && it!="." }; require(parts.isNotEmpty()); return parts.joinToString("/") { p -> require(p!=".." && ':' !in p && '\u0000' !in p) { "Unsafe path" }; p.map { if(it<' ' || it in "<>\"|?*") '_' else it }.joinToString("") } }
    fun collision(name: String,n: Int): String { if(n==0) return name; val dot=name.lastIndexOf('.').takeIf { it>0 } ?: name.length; return name.substring(0,dot)+" ($n)"+name.substring(dot) }
    data class Source(val path: String,val size: Long,val mtime: Long=0,val open: ()->InputStream)
    interface Sink { val output: OutputStream; fun commit(); fun abort() }
    fun sendText(w: Wire,text: String) { w.json(0x10,"text" to text); w.expect(0x11) }
    fun sendBatch(w: Wire,files: List<Source>,progress: (String,Int,Long)->Unit={_,_,_->}) {
        w.json(0x20,"count" to files.size,"totalBytes" to files.sumOf { it.size.coerceAtLeast(0) }); val buffer=ByteArray(BUFFER); var bytes=0L
        files.forEachIndexed { i,f -> val path=sanitize(f.path); w.json(0x21,"path" to path,"size" to f.size,"mtime" to f.mtime); var sent=0L; progress(path,i,bytes); f.open().use { input -> while(true) { val n=input.read(buffer); if(n<0) break; if(n==0) continue; w.frame(0x22,buffer,n); bytes+=n; sent+=n; progress(path,i,bytes) } }; require(f.size<0 || sent==f.size) { "Source size changed" }; w.json(0x23); progress(path,i+1,bytes) }; w.json(0x24); val ack=w.expect(0x25); require(ack["ok"]==true && (ack["saved"] as? Number)?.toInt()==files.size)
    }
    fun receive(w: Wire,sink: (String,Long)->Sink,text: (String)->Unit,start: (Int,Long)->Unit={_,_->},progress: (String,Int,Long)->Unit={_,_,_->}): Int {
        try { val first=w.read(); if(first.type==0x10) { text(first.json()["text"] as String); w.json(0x11); return 0 }; require(first.type==0x20); val m=first.json(); val count=(m["count"] as Number).toInt(); val total=(m["totalBytes"] as Number).toLong(); require(count>=0 && total>=0); start(count,total); var bytes=0L
            repeat(count) { i -> val f=w.expect(0x21); val path=sanitize(f["path"] as String); val size=(f["size"] as Number).toLong(); require(size>=-1); val target=sink(path,size); var received=0L; try { progress(path,i,bytes); while(true) { val data=w.read(); if(data.type==0x23) { data.json(); break }; require(data.type==0x22 && data.payload.size in 1..BUFFER); target.output.write(data.payload); received+=data.payload.size; bytes+=data.payload.size; require(size<0 || received<=size) { "File size mismatch" }; progress(path,i,bytes) }; require(size<0 || received==size) { "File size mismatch" }; target.output.close(); target.commit(); progress(path,i+1,bytes) } catch(e: Exception) { runCatching { target.output.close() }; target.abort(); throw e } }; w.expect(0x24); w.json(0x25,"ok" to true,"saved" to count); return count
        } catch(e: Exception) { w.error(e.message ?: "Transfer failed"); throw e }
    }
}
