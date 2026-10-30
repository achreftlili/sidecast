package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.PairingCrypto
import com.achreftlili.sidecast.protocol.constantTimeEquals
import java.net.Socket
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.SSLEngine
import javax.net.ssl.X509ExtendedTrustManager

/**
 * Trust-on-first-pairing (§2). The Mac's certificate is self-signed, so chain and hostname
 * checks are meaningless; pinning the leaf fingerprint replaces them:
 *
 *  - [pinnedFingerprint] == null (not paired yet): accept, remember the fingerprint. The pairing
 *    proof binds the user-typed code to this exact fingerprint, so a man in the middle cannot
 *    complete pairing without also guessing the code.
 *  - otherwise the leaf SHA-256 must equal the pin, or the handshake fails.
 *
 * Extends [X509ExtendedTrustManager] so the platform does not wrap it with hostname checks.
 * Lint's CustomX509TrustManager warning is expected: certificate pinning *is* the validation.
 */
@Suppress("CustomX509TrustManager")
class PinningTrustManager(private val pinnedFingerprint: ByteArray?) : X509ExtendedTrustManager() {
    @Volatile
    var observedFingerprint: ByteArray? = null
        private set

    @Volatile
    var pinMismatch: Boolean = false
        private set

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        val leaf = chain?.firstOrNull() ?: throw CertificateException("server sent no certificate")
        val fingerprint = PairingCrypto.certificateFingerprint(leaf.encoded)
        observedFingerprint = fingerprint
        val pin = pinnedFingerprint ?: return
        if (!constantTimeEquals(fingerprint, pin)) {
            pinMismatch = true
            throw CertificateException("server certificate does not match the pinned fingerprint")
        }
    }

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) =
        checkServerTrusted(chain, authType)

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) =
        checkServerTrusted(chain, authType)

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?): Unit =
        throw CertificateException("client certificates are not accepted")

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?): Unit =
        throw CertificateException("client certificates are not accepted")

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?): Unit =
        throw CertificateException("client certificates are not accepted")

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}
