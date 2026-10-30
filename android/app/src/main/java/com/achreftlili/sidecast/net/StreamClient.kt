package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.Audio
import com.achreftlili.sidecast.protocol.Auth
import com.achreftlili.sidecast.protocol.AuthResult
import com.achreftlili.sidecast.protocol.Base64Codec
import com.achreftlili.sidecast.protocol.Bye
import com.achreftlili.sidecast.protocol.ClientMessage
import com.achreftlili.sidecast.protocol.Cursor
import com.achreftlili.sidecast.protocol.FrameAck
import com.achreftlili.sidecast.protocol.FrameReader
import com.achreftlili.sidecast.protocol.Hello
import com.achreftlili.sidecast.protocol.InputMode
import com.achreftlili.sidecast.protocol.InputModeName
import com.achreftlili.sidecast.protocol.Key
import com.achreftlili.sidecast.protocol.KeyframeRequest
import com.achreftlili.sidecast.protocol.Message
import com.achreftlili.sidecast.protocol.MessageCodec
import com.achreftlili.sidecast.protocol.MicControl
import com.achreftlili.sidecast.protocol.MicState
import com.achreftlili.sidecast.protocol.PairChallenge
import com.achreftlili.sidecast.protocol.PairProof
import com.achreftlili.sidecast.protocol.PairStart
import com.achreftlili.sidecast.protocol.PairingCrypto
import com.achreftlili.sidecast.protocol.Ping
import com.achreftlili.sidecast.protocol.Pong
import com.achreftlili.sidecast.protocol.Protocol
import com.achreftlili.sidecast.protocol.ProtocolException
import com.achreftlili.sidecast.protocol.Reason
import com.achreftlili.sidecast.protocol.ServerHello
import com.achreftlili.sidecast.protocol.StreamConfig
import com.achreftlili.sidecast.protocol.Text
import com.achreftlili.sidecast.protocol.Touch
import com.achreftlili.sidecast.protocol.VideoFrame
import com.achreftlili.sidecast.protocol.constantTimeEquals
import com.achreftlili.sidecast.protocol.toHex
import com.achreftlili.sidecast.store.PairedMac
import com.achreftlili.sidecast.store.PairingRepository
import java.io.EOFException
import java.io.IOException
import java.security.SecureRandom
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/**
 * One connection attempt to a Mac, from TCP connect to close (§5–§7). Pure JVM.
 *
 * Threads:
 *  - `sidecast-rx` connects, then reads frames; VIDEO_FRAMEs are acked *before* being handed to
 *    [Listener.onVideoFrame] on this same thread (which feeds the decoder without blocking).
 *  - `sidecast-tx` is the only writer: HELLO, AUTH, acks, pings, touches, keyboard input and
 *    microphone audio go through one queue, so their bytes can never interleave and keystrokes
 *    keep their order. Audio waits in a bounded [AudioBacklog] (oldest dropped beyond ~100 ms).
 *  - `sidecast-watchdog` sends PING every second once streaming and enforces liveness / response
 *    deadlines.
 *
 * Exactly one [Listener.onEnded] is delivered per client. A client is never reused: reconnecting
 * means creating a new one (see [DisplaySession]).
 */
class StreamClient(
    private val connector: Connector,
    private val hello: Hello,
    private val pairings: PairingRepository,
    private val endpoint: Endpoint,
    private val listener: Listener,
    private val counters: RxCounters = RxCounters(),
    private val timing: Timing = Timing(),
    private val threadSetup: () -> Unit = {},
    private val log: (String) -> Unit = {},
    private val random: SecureRandom = SecureRandom(),
    /** The input mode (§7.3) to announce right after authentication; read once per connection. */
    private val inputMode: () -> String = { InputModeName.TOUCH },
) {
    data class Endpoint(val host: String, val port: Int)

    data class Timing(
        val pingIntervalMs: Long = 1_000,
        /** §6: no PONG and no VIDEO_FRAME for this long → connection lost. */
        val livenessTimeoutMs: Long = 3_000,
        /** Max wait for a reply the Mac owes us (SERVER_HELLO, PAIR_CHALLENGE, AUTH_RESULT). */
        val responseTimeoutMs: Long = 10_000,
        val watchdogTickMs: Long = 100,
    )

    enum class Phase { CONNECTING, HANDSHAKING, PAIRING, VERIFYING_CODE, AUTHENTICATING, STREAMING }

    /** The Mac is showing a code; ask the user for it. [expiresAtNanos] is on System.nanoTime(). */
    data class PairingPrompt(
        val attemptsLeft: Int,
        val expiresAtNanos: Long,
        val wrongCode: Boolean,
    )

    data class ServerIdentity(val serverId: String, val serverName: String, val newlyPaired: Boolean)

    sealed interface EndReason {
        /** [close] was called. */
        data object ClosedByUser : EndReason

        /** Could not connect / TLS failed / the Mac stopped answering before streaming. */
        data class ConnectFailed(val detail: String) : EndReason

        /** Liveness timeout or socket error after authentication. */
        data object ConnectionLost : EndReason

        /** Pinned certificate mismatch; [serverId] is known when detected after SERVER_HELLO. */
        data class IdentityChanged(val serverId: String?) : EndReason

        /** BYE, or a terminal AUTH_RESULT failure, with its §4 reason string. */
        data class ServerEnded(val reason: String) : EndReason

        data class ProtocolError(val detail: String) : EndReason
    }

    /** Callbacks arrive on the client's own threads; implementations must hand off as needed. */
    interface Listener {
        fun onPhase(phase: Phase) {}

        fun onPairingCodeRequired(prompt: PairingPrompt) {}

        fun onAuthenticated(server: ServerIdentity) {}

        fun onStreamConfig(config: StreamConfig) {}

        fun onVideoFrame(frame: VideoFrame, receivedAtNanos: Long) {}

        /** MIC_CONTROL while streaming (§7.2). */
        fun onMicControl(control: MicControl) {}

        /** CURSOR while streaming (§7.3); up to 60 per second, on the reader thread. */
        fun onCursor(cursor: Cursor) {}

        /** Connected; how the Mac was reached, when the connector knows. */
        fun onRoute(route: Route) {}

        fun onEnded(reason: EndReason) {}
    }

    private enum class State {
        IDLE, CONNECTING, AWAIT_SERVER_HELLO, AWAIT_CHALLENGE, AWAIT_CODE, AWAIT_PAIR_RESULT,
        AWAIT_AUTH_RESULT, STREAMING, ENDED,
    }

    private val lock = Any()

    @Volatile
    private var state = State.IDLE
    private val ended = AtomicBoolean(false)

    @Volatile
    private var connection: Connection? = null
    private val outbox = LinkedBlockingQueue<ByteArray>()
    private val audioBacklog = AudioBacklog()
    private var watchdog: ScheduledExecutorService? = null

    @Volatile
    private var server: ServerHello? = null

    @Volatile
    private var pairingExpiresAtNanos = 0L

    @Volatile
    private var responseDeadlineNanos = Long.MAX_VALUE
    private val lastAliveNanos = AtomicLong()
    private val lastPingNanos = AtomicLong()

    val isStreaming: Boolean get() = state == State.STREAMING

    /** AUDIO frames dropped because the network fell more than ~100 ms behind. */
    val audioFramesDropped: Long get() = audioBacklog.dropped

    fun start() {
        synchronized(lock) {
            check(state == State.IDLE) { "StreamClient is single-use" }
            state = State.CONNECTING
        }
        thread("sidecast-rx", ::runReader).start()
    }

    /** User-initiated close. Safe from any thread, any time. */
    fun close() = end(EndReason.ClosedByUser)

    /**
     * Sends PAIR_PROOF for [code]. Returns false (nothing sent) when the code is not 6 digits or
     * no code is being asked for.
     */
    fun submitPairingCode(code: String): Boolean {
        if (!PairingCrypto.isValidCode(code)) return false
        val conn: Connection
        synchronized(lock) {
            if (state != State.AWAIT_CODE) return false
            conn = connection ?: return false
            state = State.AWAIT_PAIR_RESULT
        }
        val nonce = PairingCrypto.newNonce(random)
        val proof = PairingCrypto.proof(code, conn.peerFingerprint, nonce)
        armResponseDeadline()
        listener.onPhase(Phase.VERIFYING_CODE)
        send(PairProof(nonce, proof))
        return true
    }

    fun sendTouch(touch: Touch) {
        if (state == State.STREAMING) send(touch)
    }

    /**
     * Types [text] on the Mac (§7.1), split into as many TEXT messages as its UTF-8 size needs.
     * [text] must not contain control characters: send those as [sendKey]. Dropped unless
     * streaming, like touches.
     */
    fun sendText(text: String) {
        if (state != State.STREAMING) return
        for (chunk in Text.split(text)) send(chunk)
    }

    /** Presses [key] (a [Key] constant) with [modifiers] (`Key.MOD_*` bits). Dropped unless streaming. */
    fun sendKey(key: Int, modifiers: Int = 0) {
        if (state == State.STREAMING) send(Key(key, modifiers))
    }

    fun requestKeyframe() {
        if (state == State.STREAMING) send(KeyframeRequest)
    }

    /** INPUT_MODE (§7.3) after the user toggled. Dropped unless streaming. */
    fun sendInputMode(mode: String) {
        if (state == State.STREAMING) send(InputMode(mode))
    }

    /** MIC_STATE (§7.2). Dropped unless streaming. */
    fun sendMicState(micState: MicState) {
        if (state == State.STREAMING) send(micState)
    }

    /**
     * AUDIO (§7.2). Never blocks: the frame joins the bounded [AudioBacklog] and the writer sends it
     * in turn. Dropped unless streaming.
     */
    fun sendAudio(audio: Audio) {
        if (state != State.STREAMING || ended.get()) return
        audioBacklog.offer(MessageCodec.encode(audio), audio.samples.size)
        outbox.offer(AUDIO_MARKER)
    }

    /** Forget audio not yet written (the capture stopped: nothing stale may follow MIC_STATE). */
    fun discardPendingAudio() = audioBacklog.clear()

    // ------------------------------------------------------------------ reader

    private fun runReader() {
        listener.onPhase(Phase.CONNECTING)
        val conn = try {
            connector.connect()
        } catch (e: IdentityChangedException) {
            end(EndReason.IdentityChanged(null))
            return
        } catch (e: Exception) {
            end(EndReason.ConnectFailed(describe(e)))
            return
        }
        connection = conn
        if (ended.get()) {
            conn.close() // close() raced with connect()
            return
        }
        conn.route?.let(listener::onRoute)
        thread("sidecast-tx") { runWriter(conn) }.start()
        startWatchdog()

        transition(State.AWAIT_SERVER_HELLO)
        armResponseDeadline()
        listener.onPhase(Phase.HANDSHAKING)
        send(hello)

        val reader = FrameReader(conn.input)
        try {
            while (!ended.get()) {
                val frame = reader.next() ?: throw EOFException("the Mac closed the connection")
                val receivedAt = System.nanoTime()
                val message = MessageCodec.decode(frame) ?: continue
                handle(message, conn, receivedAt, frame.payload.size)
            }
        } catch (e: ProtocolException) {
            end(EndReason.ProtocolError(e.message ?: "protocol error"))
        } catch (e: IOException) {
            end(lostReason(e))
        } catch (e: RuntimeException) {
            end(EndReason.ProtocolError("${e.javaClass.simpleName}: ${e.message}"))
        }
    }

    private fun handle(message: Message, conn: Connection, receivedAt: Long, payloadSize: Int) {
        when (message) {
            is ServerHello -> onServerHello(message, conn)
            is PairChallenge -> onPairChallenge(message, receivedAt)
            is AuthResult -> onAuthResult(message, conn)
            is StreamConfig -> {
                requireStreaming("STREAM_CONFIG")
                log("stream config $message")
                listener.onStreamConfig(message)
            }
            is VideoFrame -> {
                requireStreaming("VIDEO_FRAME")
                send(FrameAck(message.seq)) // ack on receipt, before decoding (§6)
                lastAliveNanos.set(receivedAt)
                counters.videoFrames.incrementAndGet()
                counters.videoBytes.addAndGet(payloadSize.toLong())
                listener.onVideoFrame(message, receivedAt)
            }
            is Pong -> {
                lastAliveNanos.set(receivedAt)
                val rtt = receivedAt / 1_000 - message.t
                if (rtt in 0..60_000_000) counters.rttMicros = rtt
            }
            is Bye -> end(EndReason.ServerEnded(message.reason))
            is Cursor -> if (state == State.STREAMING) listener.onCursor(message) // earlier: ignored
            is MicControl -> {
                // Only meaningful for a streaming client; anything earlier is ignored (§7.2).
                if (state == State.STREAMING) listener.onMicControl(message) else log("ignoring early MIC_CONTROL")
            }
            is ClientMessage -> log("ignoring client-direction message type 0x%02x".format(message.type))
        }
    }

    private fun onServerHello(msg: ServerHello, conn: Connection) {
        if (state != State.AWAIT_SERVER_HELLO) throw ProtocolException("unexpected SERVER_HELLO")
        if (msg.protocol != Protocol.VERSION) {
            end(EndReason.ServerEnded(Reason.PROTOCOL_MISMATCH))
            return
        }
        server = msg
        val paired = pairings.get(msg.serverId)
        if (paired != null) {
            // Pin check by serverId as well: covers connections made by IP address, where the
            // TLS layer could not know which pin to apply. The token is never sent to an
            // impostor.
            if (!constantTimeEquals(paired.fingerprint, conn.peerFingerprint)) {
                end(EndReason.IdentityChanged(msg.serverId))
                return
            }
            if (!transition(State.AWAIT_AUTH_RESULT)) return
            armResponseDeadline()
            listener.onPhase(Phase.AUTHENTICATING)
            send(Auth(hello.deviceId, paired.token))
        } else {
            if (!transition(State.AWAIT_CHALLENGE)) return
            armResponseDeadline()
            listener.onPhase(Phase.PAIRING)
            send(PairStart)
        }
    }

    private fun onPairChallenge(msg: PairChallenge, receivedAt: Long) {
        if (state != State.AWAIT_CHALLENGE && state != State.AWAIT_CODE) {
            throw ProtocolException("unexpected PAIR_CHALLENGE")
        }
        pairingExpiresAtNanos = receivedAt + TimeUnit.SECONDS.toNanos(msg.expiresInSec.toLong())
        if (!transition(State.AWAIT_CODE)) return
        responseDeadlineNanos = Long.MAX_VALUE // the user is typing; the Mac enforces expiry
        listener.onPairingCodeRequired(PairingPrompt(msg.attemptsLeft, pairingExpiresAtNanos, wrongCode = false))
    }

    private fun onAuthResult(msg: AuthResult, conn: Connection) {
        val hello = server ?: throw ProtocolException("AUTH_RESULT before SERVER_HELLO")
        when (state) {
            State.AWAIT_AUTH_RESULT -> {
                if (msg.ok) {
                    // Remember the address that answered and where the Mac says it can be reached.
                    val reached = conn.endpoint ?: endpoint
                    pairings.updateEndpoint(
                        hello.serverId,
                        hello.serverName,
                        reached.host,
                        reached.port,
                        addresses = hello.addresses,
                        serverPort = hello.port,
                    )
                    becomeStreaming(ServerIdentity(hello.serverId, hello.serverName, newlyPaired = false))
                } else {
                    val reason = msg.reason ?: Reason.UNKNOWN_DEVICE
                    // §5 AUTH: forget the stale pairing; the next attempt pairs again.
                    if (reason == Reason.UNKNOWN_DEVICE) pairings.remove(hello.serverId)
                    end(EndReason.ServerEnded(reason))
                }
            }
            State.AWAIT_PAIR_RESULT -> {
                if (msg.ok) {
                    val token = msg.token ?: throw ProtocolException("AUTH_RESULT ok without a token after pairing")
                    val reached = conn.endpoint ?: endpoint
                    pairings.save(
                        PairedMac(
                            serverId = hello.serverId,
                            serverName = hello.serverName,
                            fingerprintHex = conn.peerFingerprint.toHex(),
                            tokenB64 = Base64Codec.encode(token),
                            lastHost = reached.host,
                            lastPort = reached.port,
                            addresses = hello.addresses ?: emptyList(),
                            serverPort = hello.port ?: 0,
                        ),
                    )
                    becomeStreaming(ServerIdentity(hello.serverId, hello.serverName, newlyPaired = true))
                } else {
                    val attemptsLeft = msg.attemptsLeft ?: 0
                    when {
                        msg.reason == Reason.BAD_CODE && attemptsLeft > 0 -> {
                            if (!transition(State.AWAIT_CODE)) return
                            responseDeadlineNanos = Long.MAX_VALUE
                            listener.onPairingCodeRequired(
                                PairingPrompt(attemptsLeft, pairingExpiresAtNanos, wrongCode = true),
                            )
                        }
                        msg.reason == Reason.BAD_CODE -> end(EndReason.ServerEnded(Reason.TOO_MANY_ATTEMPTS))
                        else -> end(EndReason.ServerEnded(msg.reason ?: Reason.BAD_CODE))
                    }
                }
            }
            // The Mac may end a pairing window before any PAIR_PROOF: it expired, or another
            // phone started pairing. A failure here is terminal, not a protocol error.
            State.AWAIT_CHALLENGE, State.AWAIT_CODE -> {
                if (msg.ok) throw ProtocolException("AUTH_RESULT ok before PAIR_PROOF")
                end(EndReason.ServerEnded(msg.reason ?: Reason.PAIRING_EXPIRED))
            }
            else -> throw ProtocolException("unexpected AUTH_RESULT in state $state")
        }
    }

    private fun becomeStreaming(identity: ServerIdentity) {
        val now = System.nanoTime()
        lastAliveNanos.set(now)
        lastPingNanos.set(0)
        responseDeadlineNanos = Long.MAX_VALUE
        // Queued before STREAMING is entered, so it precedes every TOUCH of this connection.
        send(InputMode(inputMode()))
        if (!transition(State.STREAMING)) return
        log("authenticated with ${identity.serverName} (${identity.serverId}), newlyPaired=${identity.newlyPaired}")
        listener.onAuthenticated(identity)
        listener.onPhase(Phase.STREAMING)
        sendPing(now)
    }

    private fun requireStreaming(what: String) {
        if (state != State.STREAMING) throw ProtocolException("$what before authentication")
    }

    // ------------------------------------------------------------------ writer

    private fun send(message: ClientMessage) {
        if (!ended.get()) outbox.offer(MessageCodec.encode(message))
    }

    private fun runWriter(conn: Connection) {
        // An AUDIO_MARKER stands for "the oldest pending audio frame, if any is still pending".
        fun write(frame: ByteArray) {
            if (frame === AUDIO_MARKER) audioBacklog.poll()?.let(conn.output::write) else conn.output.write(frame)
        }
        try {
            while (true) {
                val first = outbox.take()
                if (first === POISON) return
                write(first)
                // Coalesce whatever queued up meanwhile into one flush (one TLS record batch).
                while (true) {
                    val next = outbox.poll() ?: break
                    if (next === POISON) {
                        conn.output.flush()
                        return
                    }
                    write(next)
                }
                conn.output.flush()
            }
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        } catch (e: IOException) {
            end(lostReason(e))
        }
    }

    // ------------------------------------------------------------------ liveness

    private fun startWatchdog() {
        val executor = Executors.newSingleThreadScheduledExecutor { r -> thread("sidecast-watchdog", r) }
        synchronized(lock) {
            if (ended.get()) {
                executor.shutdownNow()
                return
            }
            watchdog = executor
        }
        executor.scheduleWithFixedDelay(::tick, timing.watchdogTickMs, timing.watchdogTickMs, TimeUnit.MILLISECONDS)
    }

    private fun tick() {
        val now = System.nanoTime()
        when (state) {
            State.STREAMING -> {
                if (now - lastAliveNanos.get() > TimeUnit.MILLISECONDS.toNanos(timing.livenessTimeoutMs)) {
                    log("no PONG or VIDEO_FRAME for ${timing.livenessTimeoutMs} ms")
                    end(EndReason.ConnectionLost)
                } else if (now - lastPingNanos.get() >= TimeUnit.MILLISECONDS.toNanos(timing.pingIntervalMs)) {
                    sendPing(now)
                }
            }
            State.AWAIT_SERVER_HELLO, State.AWAIT_CHALLENGE, State.AWAIT_PAIR_RESULT, State.AWAIT_AUTH_RESULT -> {
                if (now > responseDeadlineNanos) end(EndReason.ConnectFailed("the Mac did not answer"))
            }
            else -> Unit
        }
    }

    private fun sendPing(now: Long) {
        lastPingNanos.set(now)
        send(Ping(now / 1_000))
    }

    private fun armResponseDeadline() {
        responseDeadlineNanos = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timing.responseTimeoutMs)
    }

    // ------------------------------------------------------------------ teardown

    private fun end(reason: EndReason) {
        if (!ended.compareAndSet(false, true)) return
        val executor: ScheduledExecutorService?
        synchronized(lock) {
            state = State.ENDED
            executor = watchdog
            watchdog = null
        }
        log("ended: $reason")
        connector.cancel()
        connection?.close()
        outbox.clear()
        audioBacklog.clear()
        outbox.offer(POISON)
        executor?.shutdownNow()
        listener.onEnded(reason)
    }

    private fun transition(to: State): Boolean = synchronized(lock) {
        if (state == State.ENDED) false else {
            state = to
            true
        }
    }

    private fun lostReason(e: Exception): EndReason = when {
        ended.get() -> EndReason.ClosedByUser // already ended; ignored by end()
        state == State.STREAMING -> EndReason.ConnectionLost
        else -> EndReason.ConnectFailed(describe(e))
    }

    private fun thread(name: String, body: Runnable) = Thread({
        threadSetup()
        body.run()
    }, name).apply { isDaemon = true }

    private fun describe(e: Throwable): String = e.message?.takeIf { it.isNotBlank() } ?: e.javaClass.simpleName

    private companion object {
        val POISON = ByteArray(0)
        val AUDIO_MARKER = ByteArray(0)
    }
}

/** Receive-side counters shared across reconnects; read by the stats overlay. */
class RxCounters {
    val videoFrames = AtomicLong()
    val videoBytes = AtomicLong()

    /** Latest PING round trip in microseconds, or -1 before the first PONG. */
    @Volatile
    var rttMicros: Long = -1
}
