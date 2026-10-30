package com.achreftlili.sidecast.protocol

import java.io.EOFException
import java.io.InputStream

/** One framed message: type byte + raw payload (§3). */
class Frame(val type: Int, val payload: ByteArray)

/** Serializes `type | length(u32 BE) | payload`. */
fun encodeFrame(type: Int, payload: ByteArray): ByteArray {
    require(type in 0..0xFF) { "type must fit in a byte" }
    require(payload.size <= Protocol.MAX_PAYLOAD) { "payload exceeds 16 MiB" }
    val out = ByteArray(Protocol.HEADER_SIZE + payload.size)
    out[0] = type.toByte()
    val len = payload.size
    out[1] = (len ushr 24).toByte()
    out[2] = (len ushr 16).toByte()
    out[3] = (len ushr 8).toByte()
    out[4] = len.toByte()
    payload.copyInto(out, Protocol.HEADER_SIZE)
    return out
}

/**
 * Incremental frame reader over a blocking [InputStream].
 *
 * Copes with arbitrarily short reads, rejects payloads above [maxPayload] *before* allocating,
 * and skips frames whose type [isKnownType] rejects (§3: "Receivers MUST ignore unknown
 * message types"). Not thread-safe: use from the single reader thread.
 */
class FrameReader(
    private val input: InputStream,
    private val maxPayload: Int = Protocol.MAX_PAYLOAD,
    private val isKnownType: (Int) -> Boolean = MessageType::isKnown,
) {
    private val header = ByteArray(Protocol.HEADER_SIZE)
    private var scratch: ByteArray? = null

    /** Number of unknown-type frames skipped so far. */
    var skippedFrames: Long = 0
        private set

    /**
     * Returns the next frame of a known type, or null on a clean end of stream at a frame
     * boundary.
     *
     * @throws EOFException if the stream ends in the middle of a frame.
     * @throws ProtocolException if a length exceeds [maxPayload].
     */
    fun next(): Frame? {
        while (true) {
            if (!readHeader()) return null
            val type = header[0].toInt() and 0xFF
            val length = ((header[1].toLong() and 0xFF) shl 24) or
                ((header[2].toLong() and 0xFF) shl 16) or
                ((header[3].toLong() and 0xFF) shl 8) or
                (header[4].toLong() and 0xFF)
            if (length > maxPayload) {
                throw ProtocolException("frame type 0x%02x length %d exceeds %d".format(type, length, maxPayload))
            }
            if (!isKnownType(type)) {
                skip(length.toInt())
                skippedFrames++
                continue
            }
            val payload = ByteArray(length.toInt())
            readFully(payload, 0, payload.size)
            return Frame(type, payload)
        }
    }

    /** False on EOF before the first header byte. */
    private fun readHeader(): Boolean {
        val first = input.read(header, 0, header.size)
        if (first < 0) return false
        if (first < header.size) readFully(header, first, header.size - first)
        return true
    }

    private fun readFully(buf: ByteArray, offset: Int, count: Int) {
        var done = 0
        while (done < count) {
            val n = input.read(buf, offset + done, count - done)
            if (n < 0) throw EOFException("stream ended inside a frame")
            done += n
        }
    }

    private fun skip(count: Int) {
        val buf = scratch ?: ByteArray(8192).also { scratch = it }
        var left = count
        while (left > 0) {
            val n = input.read(buf, 0, minOf(buf.size, left))
            if (n < 0) throw EOFException("stream ended inside a skipped frame")
            left -= n
        }
    }
}
