package com.achreftlili.sidecast.protocol

/** A protocol message. [type] is its wire type byte (§4). */
sealed interface Message {
    val type: Int
}

/** Messages sent by the phone. */
sealed interface ClientMessage : Message

/** Messages sent by the Mac. */
sealed interface ServerMessage : Message

// ---------------------------------------------------------------- client -> server

/** Physical landscape screen of the phone (width >= height). */
data class ScreenInfo(
    val width: Int,
    val height: Int,
    val densityDpi: Int,
    val refreshRate: Float,
)

/** One decoder the phone can use, with the largest landscape size it sustains at >= 60 fps. */
data class CodecSupport(
    val name: String,
    val maxWidth: Int,
    val maxHeight: Int,
)

/** HELLO `limits` (§5): caps the phone asks for, e.g. the data saver's. */
data class StreamLimits(val maxBitrateKbps: Int, val maxFps: Int)

data class Hello(
    val protocol: Int = Protocol.VERSION,
    val deviceId: String,
    val deviceName: String,
    val screen: ScreenInfo,
    val codecs: List<CodecSupport>,
    /** Optional capabilities ([Feature]); omitted from the JSON when empty. */
    val features: List<String> = emptyList(),
    /** Optional [NetworkKind] of the phone's active network. */
    val network: String? = null,
    /** Optional caps (data saver). */
    val limits: StreamLimits? = null,
) : ClientMessage {
    override val type: Int get() = MessageType.HELLO
}

class Auth(
    val deviceId: String,
    val token: ByteArray,
) : ClientMessage {
    override val type: Int get() = MessageType.AUTH

    override fun equals(other: Any?): Boolean =
        other is Auth && other.deviceId == deviceId && other.token.contentEquals(token)

    override fun hashCode(): Int = 31 * deviceId.hashCode() + token.contentHashCode()

    override fun toString(): String = "Auth(deviceId=$deviceId, token=<${token.size} bytes>)"
}

data object PairStart : ClientMessage {
    override val type: Int get() = MessageType.PAIR_START
}

class PairProof(
    val nonce: ByteArray,
    val proof: ByteArray,
) : ClientMessage {
    override val type: Int get() = MessageType.PAIR_PROOF

    override fun equals(other: Any?): Boolean =
        other is PairProof && other.nonce.contentEquals(nonce) && other.proof.contentEquals(proof)

    override fun hashCode(): Int = 31 * nonce.contentHashCode() + proof.contentHashCode()

    override fun toString(): String = "PairProof(nonce=${nonce.toHex()}, proof=${proof.toHex()})"
}

/** One touch pointer; [x]/[y] normalized to the video frame (0..1, may slightly overshoot). */
data class TouchPointer(
    val id: Int,
    val x: Float,
    val y: Float,
)

/**
 * TOUCH (§7). [eventTimeMs] is sent as u32 (wraps). [action] is the Android MotionEvent
 * masked action code.
 */
data class Touch(
    val eventTimeMs: Long,
    val action: Int,
    val actionPointerId: Int,
    val pointers: List<TouchPointer>,
) : ClientMessage {
    override val type: Int get() = MessageType.TOUCH

    companion object {
        const val ACTION_DOWN = 0
        const val ACTION_UP = 1
        const val ACTION_MOVE = 2
        const val ACTION_CANCEL = 3
        const val ACTION_POINTER_DOWN = 5
        const val ACTION_POINTER_UP = 6
    }
}

/** FRAME_ACK; [seq] is a u32. */
data class FrameAck(val seq: Long) : ClientMessage {
    override val type: Int get() = MessageType.FRAME_ACK
}

data object KeyframeRequest : ClientMessage {
    override val type: Int get() = MessageType.KEYFRAME_REQUEST
}

/** PING; [t] is client monotonic microseconds, opaque to the server. */
data class Ping(val t: Long) : ClientMessage {
    override val type: Int get() = MessageType.PING
}

/**
 * TEXT (§7.1): typed text, sent as UTF-8. On the wire it must be 1…[Protocol.MAX_TEXT_BYTES]
 * bytes and contain no control characters (those travel as [Key]); build messages for arbitrary
 * input with [split].
 */
data class Text(val text: String) : ClientMessage {
    override val type: Int get() = MessageType.TEXT

    companion object {
        /**
         * Cuts [text] into TEXT messages of at most [maxBytes] UTF-8 bytes each, only on code-point
         * boundaries (a surrogate pair is never split). Unpaired surrogates, which have no UTF-8
         * form, become U+FFFD. Empty input gives no messages.
         */
        fun split(text: String, maxBytes: Int = Protocol.MAX_TEXT_BYTES): List<Text> {
            require(maxBytes >= 4) { "maxBytes must hold any code point" }
            if (text.isEmpty()) return emptyList()
            val out = ArrayList<Text>(1)
            val chunk = StringBuilder()
            var chunkBytes = 0
            var i = 0
            while (i < text.length) {
                var cp = text.codePointAt(i)
                i += Character.charCount(cp)
                if (cp in Character.MIN_SURROGATE.code..Character.MAX_SURROGATE.code) cp = REPLACEMENT_CHARACTER
                val bytes = utf8Length(cp)
                if (chunkBytes + bytes > maxBytes) {
                    out += Text(chunk.toString())
                    chunk.setLength(0)
                    chunkBytes = 0
                }
                chunk.appendCodePoint(cp)
                chunkBytes += bytes
            }
            out += Text(chunk.toString())
            return out
        }

        private const val REPLACEMENT_CHARACTER = 0xFFFD

        private fun utf8Length(codePoint: Int): Int = when {
            codePoint < 0x80 -> 1
            codePoint < 0x800 -> 2
            codePoint < 0x10000 -> 3
            else -> 4
        }
    }
}

/**
 * AUDIO (§7.2): [samples] are 48 kHz mono PCM, sent as s16 little-endian; [seq] (u32) is 0 for the
 * first packet after each MIC_CONTROL active:true.
 */
class Audio(val seq: Long, val samples: ShortArray) : ClientMessage {
    override val type: Int get() = MessageType.AUDIO

    override fun equals(other: Any?): Boolean =
        other is Audio && other.seq == seq && other.samples.contentEquals(samples)

    override fun hashCode(): Int = 31 * seq.hashCode() + samples.contentHashCode()

    override fun toString(): String = "Audio(seq=$seq, samples=${samples.size})"
}

/** INPUT_MODE (§7.3): how the Mac interprets TOUCH ([InputModeName]); TOUCH itself is unchanged. */
data class InputMode(val mode: String) : ClientMessage {
    override val type: Int get() = MessageType.INPUT_MODE
}

/** MIC_STATE (§7.2): capturing, or not with a [MicReason]. */
data class MicState(val capturing: Boolean, val reason: String? = null) : ClientMessage {
    override val type: Int get() = MessageType.MIC_STATE
}

/** KEY (§7.1): one key press with [modifiers] held. Both are u8 on the wire. */
data class Key(val key: Int, val modifiers: Int = 0) : ClientMessage {
    override val type: Int get() = MessageType.KEY

    companion object {
        // `key` values
        const val RETURN = 1
        const val BACKSPACE = 2
        const val FORWARD_DELETE = 3
        const val TAB = 4
        const val ESCAPE = 5
        const val LEFT = 6
        const val RIGHT = 7
        const val UP = 8
        const val DOWN = 9
        const val HOME = 10
        const val END = 11
        const val PAGE_UP = 12
        const val PAGE_DOWN = 13

        // `modifiers` bits
        const val MOD_SHIFT = 1
        const val MOD_CONTROL = 2
        const val MOD_OPTION = 4
        const val MOD_COMMAND = 8
    }
}

// ---------------------------------------------------------------- server -> client

/** One SERVER_HELLO `addresses` entry; [kind] is an [AddressKind] (unknown kinds are kept). */
data class ServerAddress(val host: String, val kind: String)

data class ServerHello(
    val protocol: Int,
    val serverId: String,
    val serverName: String,
    /** The Mac's listening port, when it says (§4). */
    val port: Int? = null,
    /** Where the Mac can be reached (§1.1); null when the Mac did not say. */
    val addresses: List<ServerAddress>? = null,
) : ServerMessage {
    override val type: Int get() = MessageType.SERVER_HELLO
}

data class PairChallenge(
    val expiresInSec: Int,
    val attemptsLeft: Int,
) : ServerMessage {
    override val type: Int get() = MessageType.PAIR_CHALLENGE
}

class AuthResult(
    val ok: Boolean,
    val token: ByteArray? = null,
    val reason: String? = null,
    val attemptsLeft: Int? = null,
) : ServerMessage {
    override val type: Int get() = MessageType.AUTH_RESULT

    override fun equals(other: Any?): Boolean =
        other is AuthResult &&
            other.ok == ok &&
            other.reason == reason &&
            other.attemptsLeft == attemptsLeft &&
            (other.token?.contentEquals(token) ?: (token == null))

    override fun hashCode(): Int {
        var h = ok.hashCode()
        h = 31 * h + (token?.contentHashCode() ?: 0)
        h = 31 * h + (reason?.hashCode() ?: 0)
        h = 31 * h + (attemptsLeft ?: 0)
        return h
    }

    override fun toString(): String =
        "AuthResult(ok=$ok, token=${token?.let { "<${it.size} bytes>" }}, reason=$reason, attemptsLeft=$attemptsLeft)"
}

data class StreamConfig(
    val codec: String,
    val width: Int,
    val height: Int,
    val fps: Int,
) : ServerMessage {
    override val type: Int get() = MessageType.STREAM_CONFIG
}

/**
 * VIDEO_FRAME (§6). The access unit is `data[dataOffset until dataOffset + dataLength]`; the
 * decoder reads it in place so a received frame is never copied before MediaCodec.
 */
class VideoFrame(
    val seq: Long,
    val flags: Int,
    val ptsMicros: Long,
    val data: ByteArray,
    val dataOffset: Int = 0,
    val dataLength: Int = data.size - dataOffset,
) : ServerMessage {
    override val type: Int get() = MessageType.VIDEO_FRAME

    val isKeyframe: Boolean get() = (flags and Protocol.FLAG_KEYFRAME) != 0

    fun accessUnit(): ByteArray = data.copyOfRange(dataOffset, dataOffset + dataLength)

    override fun equals(other: Any?): Boolean =
        other is VideoFrame &&
            other.seq == seq &&
            other.flags == flags &&
            other.ptsMicros == ptsMicros &&
            other.accessUnit().contentEquals(accessUnit())

    override fun hashCode(): Int = (seq * 31 + ptsMicros).hashCode()

    override fun toString(): String =
        "VideoFrame(seq=$seq, flags=$flags, ptsMicros=$ptsMicros, bytes=$dataLength)"
}

/** PONG: the PING's 8 bytes echoed. */
data class Pong(val t: Long) : ServerMessage {
    override val type: Int get() = MessageType.PONG
}

data class Bye(val reason: String) : ServerMessage {
    override val type: Int get() = MessageType.BYE
}

/**
 * CURSOR (§7.3): the Mac's pointer, normalized to the video frame. Outside 0…1 (the server sends
 * -1,-1) means the pointer is on another display.
 */
data class Cursor(val x: Float, val y: Float) : ServerMessage {
    override val type: Int get() = MessageType.CURSOR

    /** On this display (NaN is not). */
    val onDisplay: Boolean get() = x in 0f..1f && y in 0f..1f
}

/** MIC_CONTROL (§7.2): the Mac wants the phone's microphone ([active]) or no longer does. */
data class MicControl(
    val active: Boolean,
    val sampleRate: Int? = null,
    val channels: Int? = null,
) : ServerMessage {
    override val type: Int get() = MessageType.MIC_CONTROL
}
