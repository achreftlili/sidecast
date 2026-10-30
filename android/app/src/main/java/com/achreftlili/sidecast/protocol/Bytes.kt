package com.achreftlili.sidecast.protocol

import java.security.MessageDigest
import java.util.Base64

private val HEX_DIGITS = "0123456789abcdef".toCharArray()

/** Lowercase hex, as fingerprints are shown in the protocol (§2). */
fun ByteArray.toHex(): String {
    val out = CharArray(size * 2)
    for (i in indices) {
        val v = this[i].toInt() and 0xFF
        out[i * 2] = HEX_DIGITS[v ushr 4]
        out[i * 2 + 1] = HEX_DIGITS[v and 0x0F]
    }
    return String(out)
}

fun hexToBytes(hex: String): ByteArray {
    require(hex.length % 2 == 0) { "odd hex length" }
    return ByteArray(hex.length / 2) { i ->
        val hi = Character.digit(hex[i * 2], 16)
        val lo = Character.digit(hex[i * 2 + 1], 16)
        require(hi >= 0 && lo >= 0) { "invalid hex digit" }
        ((hi shl 4) or lo).toByte()
    }
}

/** Standard Base64 with padding (RFC 4648 §4), as used for binary values inside JSON. */
object Base64Codec {
    fun encode(bytes: ByteArray): String = Base64.getEncoder().encodeToString(bytes)

    /** @throws IllegalArgumentException on malformed input. */
    fun decode(text: String): ByteArray = Base64.getDecoder().decode(text)
}

fun sha256(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)

/** Constant-time comparison. */
fun constantTimeEquals(a: ByteArray, b: ByteArray): Boolean = MessageDigest.isEqual(a, b)
