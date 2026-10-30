package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.net.StreamClient.EndReason
import com.achreftlili.sidecast.protocol.Reason
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress

class SessionPolicyTest {
    @Test
    fun backoffIsOneTwoFourThenFiveSeconds() {
        assertEquals(listOf(1_000L, 2_000L, 4_000L, 5_000L, 5_000L, 5_000L), (1..6).map(ReconnectBackoff::delayMs))
        assertEquals(5_000L, ReconnectBackoff.delayMs(1_000))
        assertEquals(1_000L, ReconnectBackoff.delayMs(0))
    }

    @Test
    fun transientFailuresRetry() {
        assertEquals(NextStep.Retry(RetryCause.CONNECTION_LOST), SessionPolicy.next(EndReason.ConnectionLost, everStreamed = true))
        assertEquals(NextStep.Retry(RetryCause.UNREACHABLE), SessionPolicy.next(EndReason.ConnectFailed("refused"), everStreamed = false))
        // Once we have streamed, a failed reconnect still reads as "connection lost".
        assertEquals(NextStep.Retry(RetryCause.CONNECTION_LOST), SessionPolicy.next(EndReason.ConnectFailed("refused"), everStreamed = true))
        assertEquals(NextStep.Retry(RetryCause.SERVER_STOPPED), SessionPolicy.next(EndReason.ServerEnded(Reason.SERVER_SHUTDOWN), true))
    }

    @Test
    fun unknownDeviceRepairsImmediately() {
        assertEquals(
            NextStep.Retry(RetryCause.REPAIR, immediate = true),
            SessionPolicy.next(EndReason.ServerEnded(Reason.UNKNOWN_DEVICE), everStreamed = false),
        )
    }

    @Test
    fun terminalReasonsNeedTheUser() {
        val expected = mapOf(
            Reason.REPLACED to SessionError.Replaced,
            Reason.PERMISSION_MISSING to SessionError.PermissionMissing,
            Reason.PROTOCOL_MISMATCH to SessionError.ProtocolMismatch,
            Reason.DISPLAY_FAILED to SessionError.DisplayFailed,
            Reason.PAIRING_EXPIRED to SessionError.PairingExpired,
            Reason.TOO_MANY_ATTEMPTS to SessionError.TooManyAttempts,
            "something_new" to SessionError.Rejected("something_new"),
        )
        for ((reason, error) in expected) {
            assertEquals(reason, NextStep.Fail(error), SessionPolicy.next(EndReason.ServerEnded(reason), everStreamed = true))
        }
        assertEquals(NextStep.Fail(SessionError.IdentityChanged), SessionPolicy.next(EndReason.IdentityChanged(null), false))
        assertEquals(NextStep.Fail(SessionError.Protocol("x")), SessionPolicy.next(EndReason.ProtocolError("x"), true))
        assertEquals(NextStep.Stop, SessionPolicy.next(EndReason.ClosedByUser, true))
    }

    @Test
    fun subnetMatching() {
        fun ip(s: String) = InetAddress.getByName(s)
        assertTrue(Subnet.contains(ip("192.168.1.0"), 24, ip("192.168.1.42")))
        assertFalse(Subnet.contains(ip("192.168.1.0"), 24, ip("192.168.2.42")))
        assertTrue(Subnet.contains(ip("10.0.0.0"), 8, ip("10.200.3.4")))
        assertTrue(Subnet.contains(ip("172.20.10.1"), 28, ip("172.20.10.14"))) // iPhone-style hotspot /28
        assertFalse(Subnet.contains(ip("172.20.10.1"), 28, ip("172.20.10.17")))
        assertTrue(Subnet.contains(ip("192.168.1.7"), 0, ip("8.8.8.8")))
        assertTrue(Subnet.contains(ip("fe80::1"), 64, ip("fe80::abcd:1")))
        assertFalse("mixed families", Subnet.contains(ip("192.168.1.0"), 24, ip("fe80::1")))
        assertFalse("bad prefix", Subnet.contains(ip("192.168.1.0"), 33, ip("192.168.1.1")))
    }
}
