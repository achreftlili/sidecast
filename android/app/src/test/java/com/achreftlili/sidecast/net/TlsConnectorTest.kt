package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.net.StreamClient.EndReason
import com.achreftlili.sidecast.protocol.AuthResult
import com.achreftlili.sidecast.protocol.Hello
import com.achreftlili.sidecast.protocol.PairChallenge
import com.achreftlili.sidecast.protocol.PairProof
import com.achreftlili.sidecast.protocol.PairStart
import com.achreftlili.sidecast.protocol.PairingCrypto
import com.achreftlili.sidecast.protocol.Protocol
import com.achreftlili.sidecast.protocol.ServerHello
import com.achreftlili.sidecast.protocol.constantTimeEquals
import com.achreftlili.sidecast.protocol.sha256
import com.achreftlili.sidecast.protocol.toHex
import org.junit.AfterClass
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeTrue
import org.junit.BeforeClass
import org.junit.Test
import java.io.File
import java.net.InetAddress
import java.nio.file.Files
import java.security.KeyStore
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLServerSocket
import javax.net.ssl.SSLSocket

/**
 * Real TLS against a self-signed EC P-256 certificate, like the Mac's. The certificate is
 * generated at test time with the JDK's keytool (no key material is checked in).
 */
class TlsConnectorTest {
    companion object {
        private const val PASSWORD = "sidecast-test"
        private lateinit var dir: File
        private var serverContext: SSLContext? = null
        private lateinit var certFingerprint: ByteArray

        @BeforeClass
        @JvmStatic
        fun createCertificate() {
            dir = Files.createTempDirectory("sidecast-tls").toFile()
            val keystore = File(dir, "mac.p12")
            val keytool = File(System.getProperty("java.home"), "bin/keytool")
            if (!keytool.canExecute()) return
            val process = ProcessBuilder(
                keytool.path, "-genkeypair", "-alias", "mac", "-keyalg", "EC", "-groupname", "secp256r1",
                "-sigalg", "SHA256withECDSA", "-dname", "CN=Sidecast Test Mac", "-validity", "3650",
                "-storetype", "PKCS12", "-keystore", keystore.path, "-storepass", PASSWORD, "-keypass", PASSWORD,
            ).redirectErrorStream(true).start()
            if (!process.waitFor(60, TimeUnit.SECONDS) || process.exitValue() != 0) return

            val ks = KeyStore.getInstance("PKCS12").apply { keystore.inputStream().use { load(it, PASSWORD.toCharArray()) } }
            certFingerprint = sha256((ks.getCertificate("mac") as X509Certificate).encoded)
            val kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm()).apply { init(ks, PASSWORD.toCharArray()) }
            serverContext = SSLContext.getInstance("TLS").apply { init(kmf.keyManagers, null, null) }
        }

        @AfterClass
        @JvmStatic
        fun cleanUp() {
            dir.deleteRecursively()
        }
    }

    /** A FakeMac whose accepted sockets are TLS. */
    private class TlsFakeMac(context: SSLContext) : FakeMac(
        (context.serverSocketFactory.createServerSocket(0, 50, InetAddress.getLoopbackAddress()) as SSLServerSocket)
            .apply { enabledProtocols = arrayOf("TLSv1.3", "TLSv1.2") },
    ) {
        override fun accept(timeoutMs: Int): Peer {
            server.soTimeout = timeoutMs
            val socket = server.accept() as SSLSocket
            socket.soTimeout = 5_000
            socket.startHandshake()
            return Peer(socket)
        }
    }

    private fun requireTls(): SSLContext {
        val ctx = serverContext
        assumeTrue("keytool unavailable: TLS tests skipped", ctx != null)
        return ctx!!
    }

    /** Runs [connect] while a background thread accepts one TLS client and tries the handshake. */
    private fun <T> withServer(connect: (Int) -> T): T {
        TlsFakeMac(requireTls()).use { mac ->
            val acceptor = Thread {
                try {
                    mac.accept().socket.getInputStream().read()
                } catch (_: Exception) {
                }
            }.apply { isDaemon = true; start() }
            try {
                return connect(mac.port)
            } finally {
                acceptor.join(5_000)
            }
        }
    }

    @Test
    fun unpairedConnectionCapturesTheLeafFingerprint() {
        val fingerprint = withServer { port ->
            TlsConnector("127.0.0.1", port, pinnedFingerprint = null).connect().use { it.peerFingerprint }
        }
        assertArrayEquals(certFingerprint, fingerprint)
    }

    @Test
    fun matchingPinIsAccepted() {
        val fingerprint = withServer { port ->
            TlsConnector("127.0.0.1", port, pinnedFingerprint = certFingerprint).connect().use { it.peerFingerprint }
        }
        assertEquals(certFingerprint.toHex(), fingerprint.toHex())
    }

    @Test
    fun mismatchingPinIsIdentityChanged() {
        withServer { port ->
            try {
                TlsConnector("127.0.0.1", port, pinnedFingerprint = sha256("another mac".toByteArray())).connect()
                fail("expected IdentityChangedException")
            } catch (_: IdentityChangedException) {
            }
        }
    }

    @Test
    fun negotiatesModernTls() {
        val protocols = TlsConnector.preferredProtocols(arrayOf("SSLv3", "TLSv1", "TLSv1.1", "TLSv1.2", "TLSv1.3"))
        assertArrayEquals(arrayOf("TLSv1.3", "TLSv1.2"), protocols)
    }

    /** §5 pairing over real TLS: the Mac verifies the proof against its *own* certificate. */
    @Test
    fun pairingProofIsBoundToTheServerCertificate() {
        TlsFakeMac(requireTls()).use { mac ->
            val pairings = InMemoryPairings()
            val listener = RecordingListener()
            val client = StreamClient(
                connector = TlsConnector("127.0.0.1", mac.port, pinnedFingerprint = null),
                hello = TEST_HELLO,
                pairings = pairings,
                endpoint = StreamClient.Endpoint("127.0.0.1", mac.port),
                listener = listener,
            )
            client.start()
            val peer = mac.accept()
            peer.expect<Hello>()
            peer.send(ServerHello(Protocol.VERSION, "srv-tls", "TLS Mac"))
            peer.expect<PairStart>()
            peer.send(PairChallenge(120, 5))
            listener.await<StreamClient.PairingPrompt>()
            assertTrue(client.submitPairingCode("424242"))
            val proof = peer.expect<PairProof>()
            val serverSide = PairingCrypto.proof("424242", certFingerprint, proof.nonce)
            assertTrue(constantTimeEquals(serverSide, proof.proof))
            peer.send(AuthResult(ok = true, token = ByteArray(32) { 1 }))
            listener.await<StreamClient.ServerIdentity>()
            assertEquals(certFingerprint.toHex(), pairings.get("srv-tls")!!.fingerprintHex)
            client.close()
            assertEquals(EndReason.ClosedByUser, listener.awaitEnd())
        }
    }
}
