package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.net.StreamClient.EndReason
import com.achreftlili.sidecast.protocol.Audio
import com.achreftlili.sidecast.protocol.Auth
import com.achreftlili.sidecast.protocol.AuthResult
import com.achreftlili.sidecast.protocol.Base64Codec
import com.achreftlili.sidecast.protocol.Cursor
import com.achreftlili.sidecast.protocol.MessageType
import com.achreftlili.sidecast.protocol.Bye
import com.achreftlili.sidecast.protocol.FrameAck
import com.achreftlili.sidecast.protocol.Hello
import com.achreftlili.sidecast.protocol.InputMode
import com.achreftlili.sidecast.protocol.InputModeName
import com.achreftlili.sidecast.protocol.Key
import com.achreftlili.sidecast.protocol.KeyframeRequest
import com.achreftlili.sidecast.protocol.MicControl
import com.achreftlili.sidecast.protocol.MicReason
import com.achreftlili.sidecast.protocol.MicState
import com.achreftlili.sidecast.protocol.PairChallenge
import com.achreftlili.sidecast.protocol.PairProof
import com.achreftlili.sidecast.protocol.PairStart
import com.achreftlili.sidecast.protocol.PairingCrypto
import com.achreftlili.sidecast.protocol.Ping
import com.achreftlili.sidecast.protocol.Pong
import com.achreftlili.sidecast.protocol.Protocol
import com.achreftlili.sidecast.protocol.Reason
import com.achreftlili.sidecast.protocol.ServerAddress
import com.achreftlili.sidecast.protocol.ServerHello
import com.achreftlili.sidecast.protocol.StreamConfig
import com.achreftlili.sidecast.protocol.Text
import com.achreftlili.sidecast.protocol.Touch
import com.achreftlili.sidecast.protocol.TouchPointer
import com.achreftlili.sidecast.protocol.VideoFrame
import com.achreftlili.sidecast.protocol.constantTimeEquals
import com.achreftlili.sidecast.protocol.encodeFrame
import com.achreftlili.sidecast.protocol.sha256
import com.achreftlili.sidecast.protocol.toHex
import com.achreftlili.sidecast.store.PairedMac
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.OutputStream
import java.net.InetAddress
import java.net.Socket

/** End-to-end protocol flows against a scripted fake Mac over loopback TCP. */
class StreamClientTest {
    private val fingerprint = sha256("fake mac certificate".toByteArray())
    private val serverId = "6a1b8a8e-1111-4222-8333-444455556666"
    private val token = ByteArray(32) { (it * 7).toByte() }
    private lateinit var mac: FakeMac
    private lateinit var pairings: InMemoryPairings
    private lateinit var listener: RecordingListener
    private var client: StreamClient? = null

    @Before
    fun setUp() {
        mac = FakeMac()
        pairings = InMemoryPairings()
        listener = RecordingListener()
    }

    @After
    fun tearDown() {
        client?.close()
        mac.close()
    }

    private fun startClient(timing: StreamClient.Timing = StreamClient.Timing()): StreamClient =
        StreamClient(
            connector = LoopbackConnector(mac.port, fingerprint),
            hello = TEST_HELLO,
            pairings = pairings,
            endpoint = StreamClient.Endpoint("127.0.0.1", mac.port),
            listener = listener,
            timing = timing,
        ).also {
            client = it
            it.start()
        }

    private fun storePairing(fp: ByteArray = fingerprint) = pairings.save(
        PairedMac(serverId, "Test Mac", fp.toHex(), Base64Codec.encode(token), "10.0.0.1", 47800),
    )

    /** Fake Mac side of AUTH with a stored token, up to AUTH_RESULT ok. */
    private fun authenticate(peer: Peer) {
        assertEquals(TEST_HELLO, peer.expect<Hello>())
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        val auth = peer.expect<Auth>()
        assertEquals(TEST_HELLO.deviceId, auth.deviceId)
        assertArrayEquals(token, auth.token)
        peer.send(AuthResult(ok = true))
    }

    @Test
    fun pairsThenStreamsWithAcksAndPings() {
        val c = startClient(StreamClient.Timing(pingIntervalMs = 100))
        val peer = mac.accept()

        val hello = peer.expect<Hello>()
        assertEquals(TEST_HELLO, hello)
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<PairStart>()
        peer.send(PairChallenge(expiresInSec = 120, attemptsLeft = 5))

        val prompt = listener.await<StreamClient.PairingPrompt>()
        assertFalse(prompt.wrongCode)
        assertEquals(5, prompt.attemptsLeft)
        assertTrue(prompt.expiresAtNanos > System.nanoTime() + 100_000_000_000L)
        assertTrue(c.submitPairingCode("123456"))
        assertFalse("only one proof per prompt", c.submitPairingCode("123456"))

        // The Mac recomputes the proof over its own certificate fingerprint.
        val proof = peer.expect<PairProof>()
        assertEquals(16, proof.nonce.size)
        assertTrue(constantTimeEquals(PairingCrypto.proof("123456", fingerprint, proof.nonce), proof.proof))
        peer.send(AuthResult(ok = true, token = token))

        val identity = listener.await<StreamClient.ServerIdentity>()
        assertEquals(StreamClient.ServerIdentity(serverId, "Test Mac", newlyPaired = true), identity)
        val stored = pairings.get(serverId)!!
        assertEquals(fingerprint.toHex(), stored.fingerprintHex)
        assertArrayEquals(token, stored.token)
        assertEquals("127.0.0.1", stored.lastHost)
        assertEquals(mac.port, stored.lastPort)

        peer.send(StreamConfig("hevc", 2400, 1080, 60))
        assertEquals(StreamConfig("hevc", 2400, 1080, 60), listener.await<StreamConfig>())

        val au = byteArrayOf(0, 0, 0, 1, 0x26, 1, 2, 3)
        peer.send(VideoFrame(0, Protocol.FLAG_KEYFRAME, 1000, au), VideoFrame(1, 0, 2000, au))
        assertEquals(0L, peer.await<FrameAck>().seq)
        assertEquals(1L, peer.await<FrameAck>().seq)
        val received = listener.await<VideoFrame>()
        assertEquals(0L, received.seq)
        assertTrue(received.isKeyframe)
        assertArrayEquals(au, received.accessUnit())

        // Pings keep flowing; PONG yields an RTT.
        val ping = peer.await<Ping>()
        peer.send(Pong(ping.t))
        peer.await<Ping>()

        c.requestKeyframe()
        peer.await<KeyframeRequest>()
        val touch = Touch(1000, Touch.ACTION_DOWN, 0, listOf(TouchPointer(0, 0.5f, 0.25f)))
        c.sendTouch(touch)
        assertEquals(touch, peer.await<Touch>())

        c.close()
        assertEquals(EndReason.ClosedByUser, listener.awaitEnd())
        assertTrue(peer.isClosedByClient())
    }

    @Test
    fun authenticatesWithStoredTokenAndHandlesReplaced() {
        storePairing()
        startClient()
        val peer = mac.accept()
        authenticate(peer)
        assertEquals(StreamClient.ServerIdentity(serverId, "Test Mac", newlyPaired = false), listener.await<StreamClient.ServerIdentity>())
        assertEquals("127.0.0.1", pairings.get(serverId)!!.lastHost)
        peer.send(Bye(Reason.REPLACED))
        assertEquals(EndReason.ServerEnded(Reason.REPLACED), listener.awaitEnd())
    }

    @Test
    fun unknownDeviceDeletesThePairing() {
        storePairing()
        startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<Auth>()
        peer.send(AuthResult(ok = false, reason = Reason.UNKNOWN_DEVICE), Bye(Reason.UNKNOWN_DEVICE))
        assertEquals(EndReason.ServerEnded(Reason.UNKNOWN_DEVICE), listener.awaitEnd())
        assertNull(pairings.get(serverId))
    }

    @Test
    fun changedCertificateNeverReceivesTheToken() {
        storePairing(fp = sha256("the real mac".toByteArray()))
        startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Impostor"))
        assertEquals(EndReason.IdentityChanged(serverId), listener.awaitEnd())
        assertTrue("no AUTH may be sent", peer.isClosedByClient())
        assertTrue(pairings.get(serverId) != null)
    }

    @Test
    fun wrongCodeRepromptsThenTooManyAttempts() {
        val c = startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<PairStart>()
        peer.send(PairChallenge(120, 5))
        listener.await<StreamClient.PairingPrompt>()

        assertFalse(c.submitPairingCode("12345"))
        assertFalse(c.submitPairingCode("abcdef"))
        assertTrue(c.submitPairingCode("000000"))
        peer.expect<PairProof>()
        peer.send(AuthResult(ok = false, reason = Reason.BAD_CODE, attemptsLeft = 4))
        val again = listener.await<StreamClient.PairingPrompt>()
        assertTrue(again.wrongCode)
        assertEquals(4, again.attemptsLeft)

        assertTrue(c.submitPairingCode("111111"))
        peer.expect<PairProof>()
        peer.send(AuthResult(ok = false, reason = Reason.BAD_CODE, attemptsLeft = 0))
        assertEquals(EndReason.ServerEnded(Reason.TOO_MANY_ATTEMPTS), listener.awaitEnd())
        assertNull(pairings.get(serverId))
    }

    @Test
    fun pairingExpiredIsReported() {
        val c = startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<PairStart>()
        peer.send(PairChallenge(120, 5))
        listener.await<StreamClient.PairingPrompt>()
        assertTrue(c.submitPairingCode("123456"))
        peer.expect<PairProof>()
        peer.send(AuthResult(ok = false, reason = Reason.PAIRING_EXPIRED), Bye(Reason.PAIRING_EXPIRED))
        assertEquals(EndReason.ServerEnded(Reason.PAIRING_EXPIRED), listener.awaitEnd())
    }

    @Test
    fun pairingExpiredWhileTypingIsReported() {
        startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<PairStart>()
        peer.send(PairChallenge(120, 5))
        listener.await<StreamClient.PairingPrompt>()
        // The Mac closes the window before the user submitted a code.
        peer.send(AuthResult(ok = false, reason = Reason.PAIRING_EXPIRED), Bye(Reason.PAIRING_EXPIRED))
        assertEquals(EndReason.ServerEnded(Reason.PAIRING_EXPIRED), listener.awaitEnd())
    }

    @Test
    fun silenceAfterAuthIsConnectionLost() {
        storePairing()
        startClient(StreamClient.Timing(pingIntervalMs = 100, livenessTimeoutMs = 400))
        val peer = mac.accept()
        authenticate(peer)
        val start = System.nanoTime()
        // Read (pings) but never answer.
        assertEquals(EndReason.ConnectionLost, listener.awaitEnd(3_000))
        val elapsedMs = (System.nanoTime() - start) / 1_000_000
        assertTrue("liveness fired after $elapsedMs ms", elapsedMs in 300..2_000)
    }

    @Test
    fun pongsKeepTheConnectionAlive() {
        storePairing()
        startClient(StreamClient.Timing(pingIntervalMs = 50, livenessTimeoutMs = 300))
        val peer = mac.accept()
        authenticate(peer)
        val until = System.currentTimeMillis() + 1_000
        while (System.currentTimeMillis() < until) {
            val ping = peer.await<Ping>()
            peer.send(Pong(ping.t))
        }
        assertTrue(listener.events.none { it is RecordingListener.Ended })
    }

    @Test
    fun serverClosingAfterAuthIsConnectionLost() {
        storePairing()
        startClient()
        val peer = mac.accept()
        authenticate(peer)
        listener.await<StreamClient.ServerIdentity>()
        peer.close()
        assertEquals(EndReason.ConnectionLost, listener.awaitEnd())
    }

    @Test
    fun serverClosingBeforeAuthIsConnectFailed() {
        startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.close()
        assertTrue(listener.awaitEnd() is EndReason.ConnectFailed)
    }

    @Test
    fun noServerIsConnectFailed() {
        val port = mac.port
        mac.close()
        StreamClient(LoopbackConnector(port, fingerprint), TEST_HELLO, pairings, StreamClient.Endpoint("127.0.0.1", port), listener)
            .also { client = it }
            .start()
        assertTrue(listener.awaitEnd() is EndReason.ConnectFailed)
    }

    @Test
    fun protocolVersionMismatch() {
        startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(2, serverId, "Future Mac"))
        assertEquals(EndReason.ServerEnded(Reason.PROTOCOL_MISMATCH), listener.awaitEnd())
    }

    @Test
    fun videoBeforeAuthIsProtocolError() {
        startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(VideoFrame(0, 1, 0, ByteArray(4)))
        assertTrue(listener.awaitEnd() is EndReason.ProtocolError)
    }

    @Test
    fun oversizeFrameIsProtocolError() {
        storePairing()
        startClient()
        val peer = mac.accept()
        authenticate(peer)
        val n = Protocol.MAX_PAYLOAD + 1
        peer.sendRaw(byteArrayOf(0x85.toByte(), (n ushr 24).toByte(), (n ushr 16).toByte(), (n ushr 8).toByte(), n.toByte()))
        assertTrue(listener.awaitEnd() is EndReason.ProtocolError)
    }

    @Test
    fun unknownMessageTypesAreSkipped() {
        storePairing()
        startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.sendRaw(encodeFrame(0x99, ByteArray(1000) { 5 }))
        peer.sendRaw(encodeFrame(0x8F, ByteArray(0)))
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<Auth>()
        peer.send(AuthResult(ok = true))
        listener.await<StreamClient.ServerIdentity>()
    }

    @Test
    fun touchesAreDroppedBeforeStreaming() {
        val c = startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        c.sendTouch(Touch(0, Touch.ACTION_DOWN, 0, listOf(TouchPointer(0, 0f, 0f))))
        c.requestKeyframe()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        // The next message must be PAIR_START, not the touch or keyframe request.
        peer.expect<PairStart>()
    }

    @Test
    fun keyboardInputIsSentInOrderOnlyWhileStreaming() {
        storePairing()
        val c = startClient()
        val peer = mac.accept()
        c.sendText("too early")
        c.sendKey(Key.RETURN)
        authenticate(peer) // HELLO then AUTH: nothing typed before streaming went out
        listener.await<StreamClient.ServerIdentity>()

        val long = "é".repeat(Protocol.MAX_TEXT_BYTES) // 8192 bytes: two TEXT messages
        c.sendText("hi")
        c.sendKey(Key.LEFT, Key.MOD_SHIFT)
        c.sendText("")
        c.sendText(long)
        c.sendKey(Key.BACKSPACE)

        val typed = ArrayList<Any>()
        while (typed.size < 5) {
            when (val m = peer.next()) {
                is Text, is Key -> typed += m
                else -> Unit // pings, acks
            }
        }
        assertEquals(Text("hi"), typed[0])
        assertEquals(Key(Key.LEFT, Key.MOD_SHIFT), typed[1])
        assertEquals(long, (typed[2] as Text).text + (typed[3] as Text).text)
        assertEquals(Key(Key.BACKSPACE), typed[4])
    }

    @Test
    fun micControlAndAudioFlowOnlyWhileStreaming() {
        storePairing()
        val c = startClient()
        val peer = mac.accept()
        assertEquals(TEST_HELLO, peer.expect<Hello>())
        peer.send(MicControl(active = true)) // before authentication: ignored, not fatal
        c.sendMicState(MicState(true))
        c.sendAudio(Audio(0, shortArrayOf(1)))
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<Auth>() // nothing mic-related went out before streaming
        peer.send(AuthResult(ok = true))
        listener.await<StreamClient.ServerIdentity>()

        peer.send(MicControl(active = true, sampleRate = 48_000, channels = 1))
        assertEquals(MicControl(true, 48_000, 1), listener.await<MicControl>())
        repeat(3) { c.sendAudio(Audio(it.toLong(), ShortArray(480) { s -> (s + it).toShort() })) }
        c.sendMicState(MicState(true))
        val received = ArrayList<Any>()
        while (received.size < 4) {
            when (val m = peer.next()) {
                is Audio, is MicState -> received += m
                else -> Unit
            }
        }
        assertEquals(listOf(0L, 1L, 2L), received.take(3).map { (it as Audio).seq })
        assertEquals(Audio(2, ShortArray(480) { s -> (s + 2).toShort() }), received[2])
        assertEquals(MicState(true), received[3])

        peer.send(MicControl(active = false))
        assertEquals(MicControl(false), listener.await<MicControl>())
        c.sendMicState(MicState(false, MicReason.STOPPED))
        assertEquals(MicState(false, MicReason.STOPPED), peer.await<MicState>())
    }

    @Test
    fun stalledNetworkDropsOldestAudioWithoutBlockingTheCaller() {
        storePairing()
        val gate = WriteGate()
        val c = StreamClient(
            connector = object : Connector {
                @Volatile
                private var socket: Socket? = null

                override fun connect(): Connection {
                    val s = Socket(InetAddress.getLoopbackAddress(), mac.port).also { socket = it }
                    return Connection(s.getInputStream(), gate.wrap(s.getOutputStream()), fingerprint) { s.close() }
                }

                override fun cancel() {
                    socket?.close()
                }
            },
            hello = TEST_HELLO,
            pairings = pairings,
            endpoint = StreamClient.Endpoint("127.0.0.1", mac.port),
            listener = listener,
        ).also {
            client = it
            it.start()
        }
        val peer = mac.accept()
        authenticate(peer)
        listener.await<StreamClient.ServerIdentity>()

        gate.close()
        c.requestKeyframe() // the writer now blocks inside this write
        waitUntil(what = "writer blocked") { gate.blocked }
        val started = System.nanoTime()
        repeat(50) { c.sendAudio(Audio(it.toLong(), ShortArray(480))) } // 500 ms of audio
        val elapsedMs = (System.nanoTime() - started) / 1_000_000
        assertTrue("sendAudio blocked for $elapsedMs ms", elapsedMs < 200)
        assertEquals(40L, c.audioFramesDropped) // only the newest 100 ms survive
        gate.open()

        val seqs = ArrayList<Long>()
        while (seqs.lastOrNull() != 49L) {
            when (val m = peer.next()) {
                is Audio -> seqs += m.seq
                is Ping -> peer.send(Pong(m.t))
                else -> Unit
            }
        }
        assertEquals((40L..49L).toList(), seqs)
    }

    /** Output stream whose writes block while the gate is closed. */
    private class WriteGate {
        private val lock = Object()
        private var closed = false

        @Volatile
        var blocked = false

        fun close() = synchronized(lock) { closed = true }

        fun open() = synchronized(lock) {
            closed = false
            lock.notifyAll()
        }

        fun wrap(out: OutputStream): OutputStream = object : OutputStream() {
            override fun write(b: Int) = write(byteArrayOf(b.toByte()), 0, 1)

            override fun write(b: ByteArray, off: Int, len: Int) {
                synchronized(lock) {
                    while (closed) {
                        blocked = true
                        lock.wait()
                    }
                    blocked = false
                }
                out.write(b, off, len)
            }

            override fun flush() = out.flush()

            override fun close() = out.close()
        }
    }

    @Test
    fun remembersTheAddressThatAnsweredAndTheAdvertisedAddresses() {
        storePairing()
        val addresses = listOf(ServerAddress("192.168.1.20", "lan"), ServerAddress("100.101.102.103", "tailscale"))
        val routes = ArrayList<Route>()
        val attempted = ArrayList<String>()
        // Stand-ins: the LAN address is down (as on mobile data); the Tailscale one answers.
        val connector = FallbackConnector(
            candidates = listOf(
                ConnectCandidate("192.168.1.20", 47800, Route.LAN, 2_500),
                ConnectCandidate("100.101.102.103", 47800, Route.TAILSCALE, 6_000),
            ),
            connectorFor = { c ->
                if (c.route == Route.LAN) {
                    object : Connector {
                        override fun connect(): Connection = throw java.net.ConnectException("unreachable")

                        override fun cancel() {}
                    }
                } else {
                    LoopbackConnector(mac.port, fingerprint, StreamClient.Endpoint(c.host, c.port))
                }
            },
            onAttempt = { attempted += it.host },
        )
        val recording = object : StreamClient.Listener by listener {
            override fun onRoute(route: Route) {
                routes += route
            }
        }
        StreamClient(connector, TEST_HELLO, pairings, StreamClient.Endpoint("192.168.1.20", 47800), recording).also {
            client = it
            it.start()
        }
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac", port = 47800, addresses = addresses))
        peer.expect<Auth>() // the pinned check passed for the Tailscale address
        peer.send(AuthResult(ok = true))
        listener.await<StreamClient.ServerIdentity>()

        assertEquals(listOf("192.168.1.20", "100.101.102.103"), attempted)
        assertEquals(listOf(Route.LAN), routes) // the loopback stand-in reports LAN
        val stored = pairings.get(serverId)!!
        assertEquals("100.101.102.103", stored.lastHost)
        assertEquals(47800, stored.lastPort)
        assertEquals(addresses, stored.addresses)
        assertEquals(47800, stored.serverPort)
    }

    @Test
    fun newPairingStoresTheAdvertisedAddresses() {
        val addresses = listOf(ServerAddress("mac.tail1.ts.net", "tailscale-dns"))
        val c = startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac", addresses = addresses))
        peer.expect<PairStart>()
        peer.send(PairChallenge(expiresInSec = 120, attemptsLeft = 5))
        listener.await<StreamClient.PairingPrompt>()
        assertTrue(c.submitPairingCode("123456"))
        peer.expect<PairProof>()
        peer.send(AuthResult(ok = true, token = token))
        listener.await<StreamClient.ServerIdentity>()
        val stored = pairings.get(serverId)!!
        assertEquals(addresses, stored.addresses)
        assertEquals(0, stored.serverPort)
        assertEquals("127.0.0.1", stored.lastHost) // no endpoint from the connector: the client's own
    }

    @Test
    fun inputModeIsAnnouncedRightAfterAuthenticationAndOnToggle() {
        storePairing()
        val mode = java.util.concurrent.atomic.AtomicReference(InputModeName.TRACKPAD)
        val c = StreamClient(
            connector = LoopbackConnector(mac.port, fingerprint),
            hello = TEST_HELLO,
            pairings = pairings,
            endpoint = StreamClient.Endpoint("127.0.0.1", mac.port),
            listener = listener,
            inputMode = { mode.get() },
        ).also {
            client = it
            it.start()
        }
        val peer = mac.accept()
        peer.expect<Hello>()
        c.sendInputMode(InputModeName.TOUCH) // not streaming yet: dropped
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<Auth>()
        peer.send(AuthResult(ok = true))
        // The very first message of the stream, ahead of PING and any TOUCH.
        assertEquals(InputMode(InputModeName.TRACKPAD), peer.expect<InputMode>())
        listener.await<StreamClient.ServerIdentity>()

        mode.set(InputModeName.TOUCH)
        c.sendInputMode(InputModeName.TOUCH)
        val touch = Touch(5, Touch.ACTION_DOWN, 0, listOf(TouchPointer(0, 0.5f, 0.5f)))
        c.sendTouch(touch)
        assertEquals(InputMode(InputModeName.TOUCH), peer.await<InputMode>())
        assertEquals(touch, peer.await<Touch>()) // applies to the touches that follow
    }

    @Test
    fun cursorIsDeliveredOnlyWhileStreamingAndBadOnesAreIgnored() {
        storePairing()
        val c = startClient()
        val peer = mac.accept()
        peer.expect<Hello>()
        peer.send(Cursor(0.1f, 0.1f)) // before authentication: ignored, not fatal
        peer.send(ServerHello(Protocol.VERSION, serverId, "Test Mac"))
        peer.expect<Auth>()
        peer.send(AuthResult(ok = true))
        listener.await<StreamClient.ServerIdentity>()

        peer.sendRaw(encodeFrame(MessageType.CURSOR, ByteArray(7))) // wrong size: ignored
        peer.send(Cursor(0.5f, 0.25f), Cursor(-1f, -1f))
        assertEquals(Cursor(0.5f, 0.25f), listener.await<Cursor>())
        assertEquals(Cursor(-1f, -1f), listener.await<Cursor>())
        assertTrue(c.isStreaming)
        assertNull(listener.events.firstOrNull { it is RecordingListener.Ended })
    }

    @Test
    fun clientMessagesAreWellFramed() {
        // Everything the writer emits must be parseable frame-by-frame (no interleaving).
        storePairing()
        val c = startClient(StreamClient.Timing(pingIntervalMs = 10))
        val peer = mac.accept()
        authenticate(peer)
        listener.await<StreamClient.ServerIdentity>()
        val threads = List(4) { n ->
            Thread {
                repeat(200) { i -> c.sendTouch(Touch(i.toLong(), Touch.ACTION_MOVE, n, List(n + 1) { TouchPointer(it, 0.1f * it, 0.2f) })) }
            }
        }
        threads.forEach(Thread::start)
        var touches = 0
        while (touches < 800) {
            when (val m = peer.next()) {
                is Touch -> touches++
                is Ping -> peer.send(Pong(m.t))
                else -> Unit
            }
        }
        threads.forEach(Thread::join)
        assertEquals(800, touches)
    }
}
