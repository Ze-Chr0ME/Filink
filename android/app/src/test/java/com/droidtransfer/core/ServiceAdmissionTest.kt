package com.droidtransfer.core

import org.junit.Assert.*
import org.junit.Test

class ServiceAdmissionTest {
    @Test fun pendingHandshakePreventsShutdown() {
        val state=ServiceAdmission()
        assertTrue(state.admit())
        assertFalse(state.tryShutdown { false })
        // Authentication does not drop the connection's admission.
        assertFalse(state.tryShutdown { false })
        assertTrue(state.finish())
        assertTrue(state.tryShutdown { false })
    }

    @Test fun admissionAfterShutdownFails() {
        val state=ServiceAdmission()
        assertTrue(state.tryShutdown { false })
        assertFalse(state.admit())
    }

    @Test fun staysBusyUntilAllConnectionsEnd() {
        val state=ServiceAdmission()
        assertTrue(state.admit())
        assertTrue(state.admit())
        assertFalse(state.finish())
        assertFalse(state.tryShutdown { false })
        assertTrue(state.finish())
        assertTrue(state.tryShutdown { false })
    }

    @Test fun receivingKeepsAdmissionOpen() {
        val state=ServiceAdmission()
        assertFalse(state.tryShutdown { true })
        assertTrue(state.admit())
        state.shutdown()
        assertFalse(state.admit())
        assertTrue(state.finish())
    }
}
