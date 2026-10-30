package com.achreftlili.sidecast.net

import java.io.Closeable
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * An established byte stream to the Mac plus the identity it proved. Production connections are
 * TLS ([TlsConnector]); tests use plain loopback sockets with a fake fingerprint.
 */
class Connection(
    val input: InputStream,
    val output: OutputStream,
    /** SHA-256 of the server's leaf certificate DER (§2). */
    val peerFingerprint: ByteArray,
    /** The address that answered (remembered as lastHost); null when the connector cannot tell. */
    val endpoint: StreamClient.Endpoint? = null,
    /** How it was reached; null when unknown. */
    val route: Route? = null,
    private val closer: () -> Unit,
) : Closeable {
    /** Abortive close; unblocks a reader or writer stuck in the socket. Idempotent. */
    override fun close() {
        try {
            closer()
        } catch (_: IOException) {
        }
    }
}

/** Opens one [Connection]. [cancel] may be called from any thread to abort a pending connect. */
interface Connector {
    /**
     * @throws IdentityChangedException when the Mac's certificate does not match the pin.
     * @throws IOException on any other failure.
     */
    fun connect(): Connection

    fun cancel()
}

/** The Mac presented a certificate other than the one pinned at pairing (§2). */
class IdentityChangedException : IOException("server certificate does not match the pinned fingerprint")
