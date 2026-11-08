package com.droidtransfer.core

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import java.io.File
import java.io.InputStream
import java.io.Closeable
import java.io.IOException

internal class TransferCancellation {
    val signal=CancellationSignal()
    @Volatile var cancelled=false
        private set
    private val resources=linkedSetOf<Closeable>()
    fun check() { if(cancelled) throw IOException("Cancelled") }
    @Synchronized fun <T: Closeable> track(resource: T): T {
        if(cancelled) { runCatching { resource.close() }; check() }
        resources.add(resource)
        return resource
    }
    @Synchronized fun untrack(resource: Closeable) { resources.remove(resource) }
    fun cancel() {
        val current=synchronized(this) { cancelled=true; resources.toList().also { resources.clear() } }
        runCatching { signal.cancel() }
        current.forEach { runCatching { it.close() } }
    }
    fun close() {
        val current=synchronized(this) { resources.toList().also { resources.clear() } }
        current.forEach { runCatching { it.close() } }
    }
}

internal class Storage(private val context: Context) {
    private val resolver=context.contentResolver
    fun source(uri: Uri,path: String?=null,cancellation: TransferCancellation=TransferCancellation()): Protocol.Source {
        cancellation.check()
        var name=uri.lastPathSegment ?: "file"; var size=-1L
        if(uri.scheme=="file") { val f=File(requireNotNull(uri.path)); name=f.name; size=f.length() }
        else resolver.query(uri,arrayOf(OpenableColumns.DISPLAY_NAME,OpenableColumns.SIZE),null,null,null,cancellation.signal)?.use { c ->
            if(c.moveToFirst()) { val ni=c.getColumnIndex(OpenableColumns.DISPLAY_NAME); val si=c.getColumnIndex(OpenableColumns.SIZE); if(ni>=0 && !c.isNull(ni)) name=c.getString(ni); if(si>=0 && !c.isNull(si)) size=c.getLong(si) }
        }
        val safe=path ?: Protocol.sanitize(name).substringAfterLast('/')
        return Protocol.Source(safe,size) {
            cancellation.check()
            val stream=if(uri.scheme=="file") File(requireNotNull(uri.path)).inputStream()
                else resolver.openAssetFileDescriptor(uri,"r",cancellation.signal)?.let { descriptor ->
                    try { descriptor.createInputStream() } catch(e: Exception) { descriptor.close(); throw e }
                } ?: error("Cannot open "+name)
            cancellation.track(stream)
            object: java.io.FilterInputStream(stream) {
                override fun close() { try { super.close() } finally { cancellation.untrack(stream) } }
            }
        }
    }
    fun tree(uri: Uri,cancellation: TransferCancellation=TransferCancellation()): List<Protocol.Source> {
        val files=mutableListOf<Protocol.Source>()
        val root=DocumentsContract.getTreeDocumentId(uri)
        fun visit(id: String,parent: String,depth: Int) {
            cancellation.check()
            require(depth<128) { "Folder nesting too deep" }
            val doc=DocumentsContract.buildDocumentUriUsingTree(uri,id)
            resolver.query(doc,arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME,DocumentsContract.Document.COLUMN_MIME_TYPE),null,null,null,cancellation.signal)?.use { c ->
                require(c.moveToFirst()); val path=if(parent.isEmpty()) c.getString(0) else parent+"/"+c.getString(0)
                if(c.getString(1)==DocumentsContract.Document.MIME_TYPE_DIR) {
                    val children=DocumentsContract.buildChildDocumentsUriUsingTree(uri,id)
                    resolver.query(children,arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID),null,null,null,cancellation.signal)?.use { child -> while(true) { cancellation.check(); if(!child.moveToNext()) break; visit(child.getString(0),path,depth+1) } }
                } else files.add(source(doc,path,cancellation))
            } ?: error("Cannot read folder")
        }
        visit(root,"",0); return files
    }
    /** Insert pending rows under the same lock as the collision query. */
    @Synchronized fun sink(path: String,size: Long,cancellation: TransferCancellation): Protocol.Sink {
        cancellation.check()
        val safe=Protocol.sanitize(path)
        val directory="Download/DroidTransfer/"+safe.substringBeforeLast('/',"").let { if(it.isEmpty()) "" else it+"/" }
        val base=safe.substringAfterLast('/')
        val collection=MediaStore.Downloads.EXTERNAL_CONTENT_URI
        val existing=hashSetOf<String>()
        resolver.query(collection,arrayOf(MediaStore.MediaColumns.DISPLAY_NAME),MediaStore.MediaColumns.RELATIVE_PATH+" = ?",arrayOf(directory),null,cancellation.signal)?.use { c ->
            while(true) { cancellation.check(); if(!c.moveToNext()) break; existing.add(c.getString(0)) }
        }
        var n=0; var name=base; while(existing.any { it.equals(name,true) }) { cancellation.check(); name=Protocol.collision(base,++n) }
        cancellation.check()
        val values=ContentValues().apply { put(MediaStore.MediaColumns.DISPLAY_NAME,name); put(MediaStore.MediaColumns.RELATIVE_PATH,directory); put(MediaStore.MediaColumns.MIME_TYPE,"application/octet-stream"); put(MediaStore.MediaColumns.IS_PENDING,1) }
        val uri=resolver.insert(collection,values) ?: error("Cannot create download")
        var descriptor: ParcelFileDescriptor?=null
        var stream: java.io.OutputStream?=null
        fun closeDestination() {
            // Close the raw handles, never the buffered wrapper: a blocked write may hold its monitor.
            descriptor?.let { runCatching { it.close() }; cancellation.untrack(it) }
            stream?.let { runCatching { it.close() }; cancellation.untrack(it) }
        }
        try {
            cancellation.check()
            val fd=cancellation.track(resolver.openFileDescriptor(uri,"w",cancellation.signal) ?: error("Cannot open download"))
            descriptor=fd
            val raw=cancellation.track(ParcelFileDescriptor.AutoCloseOutputStream(fd))
            stream=raw
            cancellation.check()
            return object: Protocol.Sink {
                override val output=raw.buffered(Protocol.BUFFER)
                // Once IS_PENDING is cleared the file is a finished download; abort() must never delete it.
                @Volatile private var published=false
                override fun commit() {
                    try { cancellation.check() } finally { closeDestination() }
                    check(resolver.update(uri,ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING,0) },null,null)>0) { "Cannot finish download" }
                    published=true
                }
                override fun abort() {
                    closeDestination()
                    if(!published) resolver.delete(uri,null,null)
                }
            }
        } catch(e: Exception) {
            closeDestination()
            runCatching { resolver.delete(uri,null,null) }
            throw e
        }
    }
}
