package com.droidtransfer.core

import org.junit.Assert.*
import org.junit.Test
import java.io.*
import java.net.*
import java.util.concurrent.*

class ProtocolTest {
    private val phone=Identity("phone-id","Phone","phone")
    private val pc=Identity("pc-id","PC","pc")
    private val key=ByteArray(32) { it.toByte() }
    private fun exchange(server: (Wire)->Unit,client: (Wire)->Unit) {
        val executor=Executors.newSingleThreadExecutor()
        ServerSocket(0,1,InetAddress.getLoopbackAddress()).use { listener ->
            val future=executor.submit<Unit> { listener.accept().use { s -> s.soTimeout=5000; server(Wire(s.getInputStream(),s.getOutputStream())) } }
            try { Socket(InetAddress.getLoopbackAddress(),listener.localPort).use { s -> s.soTimeout=5000; client(Wire(s.getInputStream(),s.getOutputStream())) }; future.get(10,TimeUnit.SECONDS) }
            finally { executor.shutdownNow() }
        }
    }
    @Test fun framingRoundTripAndBounds() {
        val out=ByteArrayOutputStream(); val w=Wire(ByteArrayInputStream(byteArrayOf()),out)
        w.frame(0x22,byteArrayOf(0,1,-1)); w.json(0x10,"text" to "héllo\n\"\\")
        assertArrayEquals(byteArrayOf(0x22,0,0,0,3,0,1,-1),out.toByteArray().copyOf(8))
        val r=Wire(ByteArrayInputStream(out.toByteArray()),ByteArrayOutputStream())
        assertArrayEquals(byteArrayOf(0,1,-1),r.read().payload); assertEquals("héllo\n\"\\",r.expect(0x10)["text"])
        assertThrows(IllegalArgumentException::class.java) { Wire(ByteArrayInputStream(byteArrayOf(1,1,0,0,1)),out).read() }
        assertThrows(EOFException::class.java) { Wire(ByteArrayInputStream(byteArrayOf(1,0,0,0,2,1)),out).read() }
    }
    @Test fun fixedCryptoVectors() {
        // Independently calculated with Python hashlib/hmac: key=00..1f,
        // client nonce=00..0f, server nonce=10..1f, raw bytes and ASCII DT1-C/DT1-S.
        val c=ByteArray(16) { it.toByte() }; val s=ByteArray(16) { (it+16).toByte() }
        assertEquals("f6fb1513c4b3b89e2540086ed31e4134e732e7e55acade3d7d0ff8b728f6f5e2",Protocol.hex(Protocol.mac(key,false,c,s)))
        assertEquals("76af1b8d03b489110fbef69b41a6c734188e943123dfac0cc8fe239b08ceaf38",Protocol.hex(Protocol.mac(key,true,c,s)))
        assertEquals("848873",Protocol.code(c,s))
    }
    @Test fun paths() {
        assertEquals("Photos/2024/a_.jpg",Protocol.sanitize("/Photos\\./2024//a?.jpg"))
        for(path in listOf("../a","a/../b","a\\..\\b","C:/a","a\u0000b","","./")) assertThrows(IllegalArgumentException::class.java) { Protocol.sanitize(path) }
        assertEquals("a (2).jpg",Protocol.collision("a.jpg",2)); assertEquals(".hidden (1)",Protocol.collision(".hidden",1))
    }
    @Test fun pairingThenAuthenticatedBatchAndTextBothWays() {
        var stored: ByteArray?=null; var serverCode=""; var clientCode=""
        exchange({ w -> assertNull(Protocol.server(w,pc,{ null },{ who,code -> assertEquals(phone,who); serverCode=code; key })) },
            { w -> val result=Protocol.client(w,phone,pc.id,null) { clientCode=it }; stored=result.second })
        assertEquals(serverCode,clientCode); assertArrayEquals(key,stored)
        val data=ByteArray(Protocol.BUFFER+79) { (it%251).toByte() }
        for(reverse in listOf(false,true)) {
            val sender=if(reverse) pc else phone; val receiver=if(reverse) phone else pc
            val outputs=linkedMapOf<String,ByteArrayOutputStream>(); var commits=0
            exchange({ w -> assertEquals(sender,Protocol.server(w,receiver,{ key })); assertEquals(2,Protocol.receive(w,{ path,_ -> val out=ByteArrayOutputStream(); outputs[path]=out; object: Protocol.Sink { override val output=out; override fun commit() { commits++ }; override fun abort() { fail("Unexpected abort") } } },{ fail("Unexpected text") })) },
                { w -> Protocol.client(w,sender,receiver.id,key); Protocol.sendBatch(w,listOf(Protocol.Source("folder/a.bin",data.size.toLong()) { ByteArrayInputStream(data) },Protocol.Source("unknown.txt",-1) { ByteArrayInputStream("hello".toByteArray()) })) })
            assertEquals(2,commits); assertArrayEquals(data,outputs["folder/a.bin"]!!.toByteArray()); assertEquals("hello",outputs["unknown.txt"].toString())
            var text=""
            exchange({ w -> Protocol.server(w,receiver,{ key }); Protocol.receive(w,{ _,_ -> error("Unexpected file") },{ text=it }) },
                { w -> Protocol.client(w,sender,receiver.id,key); Protocol.sendText(w,"Hi ☃\n") })
            assertEquals("Hi ☃\n",text)
        }
    }
    @Test fun badSizeAbortsPartial() {
        var aborted=false; var committed=false
        exchange({ w -> assertThrows(IllegalArgumentException::class.java) { Protocol.receive(w,{ _,_ -> object: Protocol.Sink { override val output=ByteArrayOutputStream(); override fun commit() { committed=true }; override fun abort() { aborted=true } } },{}) } },
            { w -> w.json(0x20,"count" to 1,"totalBytes" to 2); w.json(0x21,"path" to "a","size" to 2,"mtime" to 0); w.frame(0x22,byteArrayOf(1)); w.json(0x23); assertEquals(5,w.read().type) })
        assertTrue(aborted); assertFalse(committed)
    }
    @Test fun authenticationFailureAndPhoneRejectsPairing() {
        exchange({ w -> assertThrows(IOException::class.java) { Protocol.server(w,phone,{ key }) } },
            { w -> assertThrows(IOException::class.java) { Protocol.client(w,pc,phone.id,ByteArray(32)) } })
        exchange({ w -> assertThrows(IOException::class.java) { Protocol.server(w,phone,{ null }) } },
            { w -> w.json(1,"v" to 1,"id" to pc.id,"name" to pc.name,"kind" to "pc","nonce" to Protocol.hex(ByteArray(16))); w.expect(2); w.json(6); assertEquals(5,w.read().type) })
    }
    @Test fun unknownNestedJsonFields() {
        assertEquals("ok",Json.parse("""{"future":{"a":[1,true,null,{"nested":"yes"}]},"text":"ok"}""")["text"])
    }
    @Test fun disconnectAbortsPartialAndTraversalNeverCreatesSink() {
        for(path in listOf("a.bin","../escape")) {
            val out=ByteArrayOutputStream(); val w=Wire(ByteArrayInputStream(byteArrayOf()),out)
            w.json(0x20,"count" to 1,"totalBytes" to 4); w.json(0x21,"path" to path,"size" to 4,"mtime" to 0); w.frame(0x22,byteArrayOf(1,2))
            var opened=false; var aborted=false
            val r=Wire(ByteArrayInputStream(out.toByteArray()),ByteArrayOutputStream())
            assertThrows(Exception::class.java) { Protocol.receive(r,{ _,_ -> opened=true; object: Protocol.Sink { override val output=ByteArrayOutputStream(); override fun commit() { fail() }; override fun abort() { aborted=true } } },{}) }
            assertEquals(path=="a.bin",opened); assertEquals(path=="a.bin",aborted)
        }
    }
}
