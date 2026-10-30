package com.achreftlili.sidecast.protocol

import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** Pairing cryptography (§5 "Pairing"). */
object PairingCrypto {
    private const val HMAC = "HmacSHA256"

    /** Certificate fingerprint = SHA-256 of the leaf certificate's DER encoding (§2). */
    fun certificateFingerprint(der: ByteArray): ByteArray = sha256(der)

    /**
     * proof = HMAC-SHA256(key = ASCII(code), message = certFingerprint(32) || nonce(16)).
     */
    fun proof(code: String, certFingerprint: ByteArray, nonce: ByteArray): ByteArray {
        require(isValidCode(code)) { "pairing code must be 6 ASCII digits" }
        require(certFingerprint.size == Protocol.FINGERPRINT_SIZE) { "fingerprint must be 32 bytes" }
        require(nonce.size == Protocol.NONCE_SIZE) { "nonce must be 16 bytes" }
        val mac = Mac.getInstance(HMAC)
        mac.init(SecretKeySpec(code.toByteArray(Charsets.US_ASCII), HMAC))
        mac.update(certFingerprint)
        mac.update(nonce)
        return mac.doFinal()
    }

    /** What the server stores for a device: SHA-256(token). Client side only used in tests. */
    fun tokenHash(token: ByteArray): ByteArray = sha256(token)

    fun newNonce(random: SecureRandom = SecureRandom()): ByteArray =
        ByteArray(Protocol.NONCE_SIZE).also { random.nextBytes(it) }

    fun isValidCode(code: String): Boolean =
        code.length == Protocol.PAIRING_CODE_LENGTH && code.all { it in '0'..'9' }
}
