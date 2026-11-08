package com.droidtransfer.core

import org.junit.Assert.*
import org.junit.Test

class HandshakeDeadlineTest {
    @Test fun slowHandshakeCannotExtendAbsoluteDeadline() {
        val deadline=HandshakeDeadline(100,10000)
        assertFalse(deadline.expire(1000))
        assertFalse(deadline.expire(10099))
        assertFalse(deadline.authenticate(10100))
        assertTrue(deadline.expire(10100))
        assertFalse(deadline.authenticate(10101))
    }
    @Test fun authenticatedConnectionSurvivesTimeoutCallback() {
        val deadline=HandshakeDeadline(100,10000)
        assertTrue(deadline.authenticate(10099))
        assertFalse(deadline.expire(10100))
    }
    @Test fun timeoutWinsRaceWithAuthentication() {
        val deadline=HandshakeDeadline(100,10000)
        assertTrue(deadline.expire(10100))
        assertFalse(deadline.expire(10101))
        assertFalse(deadline.authenticate(10101))
    }
}
