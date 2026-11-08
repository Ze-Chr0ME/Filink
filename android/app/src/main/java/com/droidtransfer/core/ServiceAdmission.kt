package com.droidtransfer.core

/** Counts admitted connections (including authentication) until their final cleanup. */
internal class ServiceAdmission {
    private var busy=0
    private var stopping=false

    @Synchronized fun admit(): Boolean {
        if(stopping) return false
        busy++
        return true
    }

    @Synchronized fun finish(): Boolean {
        check(busy>0)
        return --busy==0
    }

    /** The decision and closing admission happen atomically. */
    @Synchronized fun tryShutdown(keepReceiving: ()->Boolean): Boolean {
        if(stopping || busy!=0 || keepReceiving()) return false
        stopping=true
        return true
    }

    @Synchronized fun shutdown() { stopping=true }
}
