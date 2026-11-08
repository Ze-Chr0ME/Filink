package com.droidtransfer.core

/** A monotonic, absolute deadline; authentication and the timeout race through one lock. */
internal class HandshakeDeadline(private val acceptedAt: Long,private val timeout: Long) {
    private var finished=false
    @Synchronized fun authenticate(now: Long): Boolean {
        if(finished || now-acceptedAt>=timeout) return false
        finished=true
        return true
    }
    @Synchronized fun expire(now: Long): Boolean {
        if(finished || now-acceptedAt<timeout) return false
        finished=true
        return true
    }
}
