package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.PairingCrypto
import com.achreftlili.sidecast.protocol.constantTimeEquals
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.security.cert.X509Certificate
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLException
import javax.net.ssl.SSLPeerUnverifiedException
import javax.net.ssl.SSLSocket

/**
 * TCP (Nagle off) + TLS 1.2/1.3 to the Mac with [PinningTrustManager]. Pure JVM so it can be
 * exercised against a local TLS server in unit tests; Android-specific socket routing is
 * injected through [prepareSocket].
 */
class TlsConnector(
    private val host: String,
    private val port: Int,
    private val pinnedFingerprint: ByteArray?,
    /** Called on the unconnected socket, e.g. to bind it to the Wi-Fi network. */
    private val prepareSocket: (Socket, InetAddress) -> Unit = { _, _ -> },
    private val connectTimeoutMs: Int = 5_000,
    private val handshakeTimeoutMs: Int = 10_000,
) : Connector {
    @Volatile
    private var pending: Socket? = null

    @Volatile
    private var cancelled = false

    override fun connect(): Connection {
        val address = InetAddress.getByName(host)
        val raw = Socket()
        pending = raw
        try {
            if (cancelled) throw IOException("cancelled")
            prepareSocket(raw, address)
            raw.tcpNoDelay = true
            raw.connect(InetSocketAddress(address, port), connectTimeoutMs)
            raw.soTimeout = handshakeTimeoutMs

            // A fresh context per connection: no TLS session resumption across pin changes.
            val trust = PinningTrustManager(pinnedFingerprint)
            val context = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), null) }
            val ssl = context.socketFactory.createSocket(raw, host, port, true) as SSLSocket
            ssl.useClientMode = true
            ssl.enabledProtocols = preferredProtocols(ssl.supportedProtocols)
            try {
                ssl.startHandshake()
            } catch (e: IOException) {
                if (trust.pinMismatch) throw IdentityChangedException()
                throw e
            }

            val leaf = ssl.session.peerCertificates.firstOrNull() as? X509Certificate
                ?: throw SSLPeerUnverifiedException("server sent no certificate")
            val fingerprint = PairingCrypto.certificateFingerprint(leaf.encoded)
            if (pinnedFingerprint != null && !constantTimeEquals(fingerprint, pinnedFingerprint)) {
                throw IdentityChangedException()
            }
            // Liveness is enforced by the protocol (PING/PONG), not by socket timeouts.
            ssl.soTimeout = 0

            return Connection(
                input = BufferedInputStream(ssl.inputStream, 64 * 1024),
                output = BufferedOutputStream(ssl.outputStream, 16 * 1024),
                peerFingerprint = fingerprint,
                endpoint = StreamClient.Endpoint(host, port),
                route = AddressClass.route(address),
                // Closing the raw socket is abortive and reliably unblocks threads stuck in
                // SSL reads/writes (same technique OkHttp uses for cancellation).
                closer = { raw.close() },
            )
        } catch (e: Throwable) {
            closeQuietly(raw)
            if (cancelled && e !is IdentityChangedException) throw IOException("cancelled", e)
            throw e
        } finally {
            pending = null
        }
    }

    override fun cancel() {
        cancelled = true
        pending?.let(::closeQuietly)
    }

    private fun closeQuietly(socket: Socket) {
        try {
            socket.close()
        } catch (_: IOException) {
        }
    }

    companion object {
        /** TLS 1.2 minimum, 1.3 preferred (§2). */
        fun preferredProtocols(supported: Array<String>): Array<String> {
            val wanted = listOf("TLSv1.3", "TLSv1.2").filter { it in supported }
            if (wanted.isEmpty()) throw SSLException("TLS 1.2+ is not available")
            return wanted.toTypedArray()
        }
    }
}
