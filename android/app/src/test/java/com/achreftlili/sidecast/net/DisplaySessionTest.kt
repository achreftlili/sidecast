package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.Auth
import com.achreftlili.sidecast.protocol.AuthResult
import com.achreftlili.sidecast.protocol.Base64Codec
import com.achreftlili.sidecast.protocol.Bye
import com.achreftlili.sidecast.protocol.Audio
import com.achreftlili.sidecast.protocol.Cursor
import com.achreftlili.sidecast.protocol.Hello
import com.achreftlili.sidecast.protocol.HostPlatform
import com.achreftlili.sidecast.protocol.InputMode
import com.achreftlili.sidecast.protocol.InputModeName
import com.achreftlili.sidecast.protocol.MicControl
import com.achreftlili.sidecast.protocol.MicState
import com.achreftlili.sidecast.protocol.PairStart
import com.achreftlili.sidecast.protocol.Protocol
import com.achreftlili.sidecast.protocol.Reason
import com.achreftlili.sidecast.protocol.ServerHello
import com.achreftlili.sidecast.protocol.StreamConfig
import com.achreftlili.sidecast.protocol.VideoFrame
import com.achreftlili.sidecast.protocol.sha256
import com.achreftlili.sidecast.protocol.toHex
import com.achreftlili.sidecast.store.PairedMac
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import java.net.SocketTimeoutException
import java.util.concurrent.CopyOnWriteArrayList

/** Reconnect / failure behaviour of the session loop around real StreamClients. */
class DisplaySessionTest {
    private val fingerprint = sha256("session mac".toByteArray())
    private val serverId = "srv-session"
    private val token = ByteArray(32) { 3 }
    private lateinit var mac: FakeMac
    private lateinit var pairings: InMemoryPairings
    private lateinit var session: DisplaySession
    private val configs = CopyOnWriteArrayList<StreamConfig>()
    private val frames = CopyOnWriteArrayList<Long>()
    private val pins = CopyOnWriteArrayList<String?>()

    private val sink = object : VideoSink {
        override fun onStreamConfig(config: StreamConfig) {
            configs += config
        }

        override fun onVideoFrame(frame: VideoFrame, receivedAtNanos: Long) {
            frames += frame.seq
        }
    }

    @Before
    fun setUp() {
        mac = FakeMac()
        pairings = InMemoryPairings()
        pairings.save(PairedMac(serverId, "Session Mac", fingerprint.toHex(), Base64Codec.encode(token), "127.0.0.1", mac.port))
        session = DisplaySession("Session Mac", serverId, pairings, sink, { expected, counters, listener ->
            pins += expected?.let { pairings.get(it)?.fingerprintHex }
            StreamClient(
                LoopbackConnector(mac.port, fingerprint),
                TEST_HELLO,
                pairings,
                StreamClient.Endpoint("127.0.0.1", mac.port),
                listener,
                counters,
                inputMode = { session.inputMode },
            )
        })
    }

    @After
    fun tearDown() {
        session.stop()
        mac.close()
    }

    private fun authenticate(peer: Peer) {
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Session Mac"))
        peer.expect<Auth>()
        peer.send(AuthResult(ok = true))
    }

    private fun awaitState(what: String, predicate: (SessionState) -> Boolean) =
        waitUntil(what = "$what (now ${session.state.value})") { predicate(session.state.value) }

    @Test
    fun streamsAndRoutesVideoToTheSink() {
        session.start()
        val peer = mac.accept()
        authenticate(peer)
        awaitState("streaming") { it is SessionState.Streaming }
        peer.send(StreamConfig("h264", 1920, 1080, 60), VideoFrame(0, 1, 0, byteArrayOf(0, 0, 1, 0x65)))
        waitUntil(what = "frame") { frames.isNotEmpty() }
        assertEquals(StreamConfig("h264", 1920, 1080, 60), session.streamConfig.value)
        assertEquals(listOf(StreamConfig("h264", 1920, 1080, 60)), configs.toList())
        assertEquals(fingerprint.toHex(), pins.first())
        assertEquals(1L, session.counters.videoFrames.get())
    }

    @Test
    fun knowsTheHostPlatformOnceAuthenticated() {
        assertNull(session.platform)
        session.start()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Session Mac", platform = HostPlatform.WINDOWS))
        peer.expect<Auth>()
        peer.send(AuthResult(ok = true))
        awaitState("streaming") { it is SessionState.Streaming }
        assertEquals(HostPlatform.WINDOWS, session.platform)
    }

    @Test
    fun micRequestsRouteToTheHandlerAndEndWithTheConnection() {
        val mic = CopyOnWriteArrayList<Any>()
        session.micHandler = object : MicHandler {
            override fun onMicControl(control: MicControl) {
                mic += control
            }

            override fun onMicSessionEnded() {
                mic += "ended"
            }
        }
        session.start()
        val first = mac.accept()
        authenticate(first)
        awaitState("streaming") { it is SessionState.Streaming }
        first.send(MicControl(active = true))
        waitUntil(what = "mic control") { mic.isNotEmpty() }
        assertEquals(MicControl(active = true), mic[0])

        // MIC_STATE and AUDIO go back to the connection that asked.
        session.sendAudio(Audio(0, shortArrayOf(1, 2)))
        session.sendMicState(MicState(true))
        assertEquals(Audio(0, shortArrayOf(1, 2)), first.await<Audio>())
        assertEquals(MicState(true), first.await<MicState>())

        first.close() // the connection drops: the mic must stop, the next session starts with it off
        waitUntil(what = "mic session end") { mic.size == 2 }
        assertEquals("ended", mic[1])
        authenticate(mac.accept(timeoutMs = 3_000))
        awaitState("streaming again") { it is SessionState.Streaming }
        session.sendMicState(MicState(true)) // no owner any more: dropped
        assertEquals(2, mic.size)
    }

    @Test
    fun inputModeIsSentOnEveryConnectAndOnToggleOnlyWhileStreaming() {
        session.setInputMode(InputModeName.TRACKPAD) // the saved choice, before connecting
        session.start()
        val first = mac.accept()
        authenticate(first)
        assertEquals(InputMode(InputModeName.TRACKPAD), first.await<InputMode>())
        awaitState("streaming") { it is SessionState.Streaming }

        session.setInputMode(InputModeName.TOUCH)
        assertEquals(InputMode(InputModeName.TOUCH), first.await<InputMode>())

        first.close() // reconnect: the current mode is announced again
        val second = mac.accept(timeoutMs = 3_000)
        session.setInputMode(InputModeName.TRACKPAD) // toggled while reconnecting: not sent now...
        authenticate(second)
        assertEquals(InputMode(InputModeName.TRACKPAD), second.await<InputMode>()) // ...but on auth
        assertEquals(InputModeName.TRACKPAD, session.inputMode)
    }

    @Test
    fun cursorFollowsTheConnectionAndResetsOnModeChangeAndReconnect() {
        session.setInputMode(InputModeName.TRACKPAD)
        session.start()
        val first = mac.accept()
        authenticate(first)
        awaitState("streaming") { it is SessionState.Streaming }
        assertNull(session.cursor.value) // nothing until the first CURSOR
        first.send(Cursor(0.25f, 0.75f), Cursor(0.5f, 0.5f))
        waitUntil(what = "latest cursor") { session.cursor.value == Cursor(0.5f, 0.5f) }

        session.setInputMode(InputModeName.TOUCH)
        assertNull("stale position dropped on a mode change", session.cursor.value)
        session.setInputMode(InputModeName.TRACKPAD)
        first.send(Cursor(0.1f, 0.2f))
        waitUntil(what = "cursor after switching back") { session.cursor.value == Cursor(0.1f, 0.2f) }

        first.close()
        awaitState("reconnecting") { it is SessionState.Reconnecting }
        assertNull("no stale pointer while reconnecting", session.cursor.value)
    }

    @Test
    fun lostConnectionReconnectsAfterBackoff() {
        session.start()
        val first = mac.accept()
        authenticate(first)
        awaitState("streaming") { it is SessionState.Streaming }
        first.close()
        awaitState("reconnecting") { it == SessionState.Reconnecting("Session Mac", RetryCause.CONNECTION_LOST, 1) }
        val lostAt = System.nanoTime()
        val second = mac.accept(timeoutMs = 3_000)
        val waitedMs = (System.nanoTime() - lostAt) / 1_000_000
        assertTrue("reconnected after $waitedMs ms", waitedMs in 700..2_500)
        authenticate(second)
        awaitState("streaming again") { it is SessionState.Streaming }
        assertEquals("the reconnect pins the paired certificate", fingerprint.toHex(), pins.last())
    }

    @Test
    fun replacedDoesNotReconnect() {
        session.start()
        val peer = mac.accept()
        authenticate(peer)
        awaitState("streaming") { it is SessionState.Streaming }
        peer.send(Bye(Reason.REPLACED))
        awaitState("failed") { it == SessionState.Failed("Session Mac", SessionError.Replaced) }
        try {
            mac.accept(timeoutMs = 1_500)
            fail("must not reconnect after replaced")
        } catch (_: SocketTimeoutException) {
        }
        // The user can reconnect explicitly.
        session.retryNow()
        authenticate(mac.accept())
        awaitState("streaming again") { it is SessionState.Streaming }
    }

    @Test
    fun unknownDeviceForgetsAndPairsAgainImmediately() {
        session.start()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Session Mac"))
        peer.expect<Auth>()
        peer.send(AuthResult(ok = false, reason = Reason.UNKNOWN_DEVICE))
        val again = mac.accept(timeoutMs = 1_000) // no backoff
        assertNull(pairings.get(serverId))
        again.expect<Hello>()
        again.send(ServerHello(Protocol.VERSION, serverId, "Session Mac"))
        again.expect<PairStart>()
        awaitState("pairing") { it is SessionState.Pairing }
    }

    @Test
    fun stopIsTerminal() {
        session.start()
        authenticate(mac.accept())
        awaitState("streaming") { it is SessionState.Streaming }
        session.stop()
        awaitState("stopped") { it == SessionState.Stopped }
        session.retryNow()
        try {
            mac.accept(timeoutMs = 1_500)
            fail("stopped session reconnected")
        } catch (_: SocketTimeoutException) {
        }
        assertTrue(session.state.value == SessionState.Stopped)
    }
}
