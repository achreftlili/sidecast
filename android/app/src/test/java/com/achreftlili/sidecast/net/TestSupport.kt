package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.CodecSupport
import com.achreftlili.sidecast.protocol.FrameReader
import com.achreftlili.sidecast.protocol.Hello
import com.achreftlili.sidecast.protocol.Message
import com.achreftlili.sidecast.protocol.MessageCodec
import com.achreftlili.sidecast.protocol.ScreenInfo
import com.achreftlili.sidecast.store.PairedMac
import com.achreftlili.sidecast.store.PairingRepository
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.Closeable
import java.io.EOFException
import java.io.IOException
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.Assert.fail

val TEST_HELLO = Hello(
    deviceId = "11111111-2222-4333-8444-555555555555",
    deviceName = "Test Phone",
    screen = ScreenInfo(2400, 1080, 420, 120f),
    codecs = listOf(CodecSupport("hevc", 3840, 2160), CodecSupport("h264", 1920, 1080)),
)

class InMemoryPairings : PairingRepository {
    private val map = ConcurrentHashMap<String, PairedMac>()

    override fun get(serverId: String): PairedMac? = map[serverId]

    override fun all(): List<PairedMac> = map.values.toList()

    override fun save(mac: PairedMac) {
        map[mac.serverId] = mac
    }

    override fun remove(serverId: String) {
        map.remove(serverId)
    }
}

/** Plain TCP to a local fake Mac; the "certificate fingerprint" is supplied by the test. */
class LoopbackConnector(
    private val port: Int,
    private val fingerprint: ByteArray,
    /** Reported as the address that answered (what a TlsConnector reports), if set. */
    private val endpoint: StreamClient.Endpoint? = null,
) : Connector {
    @Volatile
    private var socket: Socket? = null

    override fun connect(): Connection {
        val s = Socket(InetAddress.getLoopbackAddress(), port)
        s.tcpNoDelay = true
        socket = s
        return Connection(
            BufferedInputStream(s.getInputStream()),
            BufferedOutputStream(s.getOutputStream()),
            fingerprint,
            endpoint,
            endpoint?.let { Route.LAN },
        ) { s.close() }
    }

    override fun cancel() {
        socket?.close()
    }
}

/** A scriptable Mac: accept connections, read typed messages, send messages. */
open class FakeMac(protected val server: ServerSocket = ServerSocket(0, 50, InetAddress.getLoopbackAddress())) : Closeable {
    val port: Int get() = server.localPort

    open fun accept(timeoutMs: Int = 5_000): Peer {
        server.soTimeout = timeoutMs
        return Peer(server.accept())
    }

    override fun close() = server.close()
}

class Peer(val socket: Socket) : Closeable {
    init {
        socket.soTimeout = 5_000
    }

    private val reader = FrameReader(socket.getInputStream())
    private val out = socket.getOutputStream()

    fun next(): Message {
        val frame = reader.next() ?: throw EOFException("client closed")
        return MessageCodec.decode(frame) ?: error("unknown type ${frame.type}")
    }

    /** Next message, which must be a [T]. */
    inline fun <reified T : Message> expect(): T {
        val m = next()
        if (m !is T) fail("expected ${T::class.simpleName}, got $m")
        return m as T
    }

    /** Skips other messages (pings, acks) until a [T] arrives. */
    inline fun <reified T : Message> await(): T {
        while (true) {
            val m = next()
            if (m is T) return m
        }
    }

    /** True when the client closed the connection without sending anything else. */
    fun isClosedByClient(): Boolean = try {
        reader.next() == null
    } catch (_: IOException) {
        true
    }

    fun send(vararg messages: Message) {
        for (m in messages) out.write(MessageCodec.encode(m))
        out.flush()
    }

    fun sendRaw(bytes: ByteArray) {
        out.write(bytes)
        out.flush()
    }

    override fun close() = socket.close()
}

/** Records listener callbacks for assertions. */
class RecordingListener : StreamClient.Listener {
    val events = LinkedBlockingQueue<Any>()

    override fun onPhase(phase: StreamClient.Phase) {
        events.add(phase)
    }

    override fun onPairingCodeRequired(prompt: StreamClient.PairingPrompt) {
        events.add(prompt)
    }

    override fun onAuthenticated(server: StreamClient.ServerIdentity) {
        events.add(server)
    }

    override fun onStreamConfig(config: com.achreftlili.sidecast.protocol.StreamConfig) {
        events.add(config)
    }

    override fun onVideoFrame(frame: com.achreftlili.sidecast.protocol.VideoFrame, receivedAtNanos: Long) {
        events.add(frame)
    }

    override fun onMicControl(control: com.achreftlili.sidecast.protocol.MicControl) {
        events.add(control)
    }

    override fun onCursor(cursor: com.achreftlili.sidecast.protocol.Cursor) {
        events.add(cursor)
    }

    override fun onEnded(reason: StreamClient.EndReason) {
        events.add(Ended(reason))
    }

    data class Ended(val reason: StreamClient.EndReason)

    inline fun <reified T : Any> await(timeoutMs: Long = 5_000): T {
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
        while (true) {
            val left = deadline - System.nanoTime()
            val e = events.poll(left.coerceAtLeast(0), TimeUnit.NANOSECONDS)
                ?: throw AssertionError("timed out waiting for ${T::class.simpleName}")
            if (e is T) return e
        }
    }

    fun awaitEnd(timeoutMs: Long = 5_000): StreamClient.EndReason = await<Ended>(timeoutMs).reason
}

fun waitUntil(timeoutMs: Long = 5_000, what: String, condition: () -> Boolean) {
    val deadline = System.currentTimeMillis() + timeoutMs
    while (!condition()) {
        if (System.currentTimeMillis() > deadline) fail("timed out waiting for $what")
        Thread.sleep(10)
    }
}
