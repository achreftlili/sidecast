package com.achreftlili.sidecast.protocol

/** Constants of Sidecast wire protocol v1 (docs/PROTOCOL.md). Pure Kotlin: no Android imports. */
object Protocol {
    const val VERSION = 1
    const val DEFAULT_PORT = 47800
    const val SERVICE_TYPE = "_sidecast._tcp"
    const val TXT_ID = "id"
    const val TXT_VERSION = "v"
    const val TXT_OS = "os"

    /** type (u8) + length (u32). */
    const val HEADER_SIZE = 5

    /** Largest accepted payload; anything above is a protocol error (§3). */
    const val MAX_PAYLOAD = 16 * 1024 * 1024

    const val FINGERPRINT_SIZE = 32
    const val NONCE_SIZE = 16
    const val PROOF_SIZE = 32
    const val PAIRING_CODE_LENGTH = 6
    const val MAX_TOUCH_POINTERS = 10

    /** TEXT payload size bounds in UTF-8 bytes (§7.1). */
    const val MIN_TEXT_BYTES = 1
    const val MAX_TEXT_BYTES = 4096

    /** Microphone audio (§7.2): PCM s16le mono at 48 kHz, 1…4800 samples per AUDIO message. */
    const val MIC_SAMPLE_RATE = 48_000
    const val MIC_CHANNELS = 1
    const val MAX_AUDIO_SAMPLES = 4800

    /** Recommended AUDIO size: 10 ms. */
    const val MIC_CHUNK_SAMPLES = 480

    /** VIDEO_FRAME flags bit 0. */
    const val FLAG_KEYFRAME = 0x01
}

/** Message type bytes (§4). */
object MessageType {
    // Client -> server
    const val HELLO = 0x01
    const val AUTH = 0x02
    const val PAIR_START = 0x03
    const val PAIR_PROOF = 0x04
    const val TOUCH = 0x10
    const val FRAME_ACK = 0x11
    const val KEYFRAME_REQUEST = 0x12
    const val PING = 0x13
    const val TEXT = 0x14
    const val KEY = 0x15
    const val AUDIO = 0x16
    const val MIC_STATE = 0x17
    const val INPUT_MODE = 0x18

    // Server -> client
    const val SERVER_HELLO = 0x81
    const val PAIR_CHALLENGE = 0x82
    const val AUTH_RESULT = 0x83
    const val STREAM_CONFIG = 0x84
    const val VIDEO_FRAME = 0x85
    const val PONG = 0x86
    const val BYE = 0x87
    const val MIC_CONTROL = 0x88
    const val CURSOR = 0x89

    private val KNOWN = setOf(
        HELLO, AUTH, PAIR_START, PAIR_PROOF, TOUCH, FRAME_ACK, KEYFRAME_REQUEST, PING, TEXT, KEY, AUDIO, MIC_STATE, INPUT_MODE,
        SERVER_HELLO, PAIR_CHALLENGE, AUTH_RESULT, STREAM_CONFIG, VIDEO_FRAME, PONG, BYE, MIC_CONTROL, CURSOR,
    )

    fun isKnown(type: Int): Boolean = type in KNOWN
}

/** `reason` strings carried by AUTH_RESULT and BYE (§4). */
object Reason {
    const val BAD_CODE = "bad_code"
    const val TOO_MANY_ATTEMPTS = "too_many_attempts"
    const val PAIRING_EXPIRED = "pairing_expired"
    const val UNKNOWN_DEVICE = "unknown_device"
    const val PROTOCOL_MISMATCH = "protocol_mismatch"
    const val REPLACED = "replaced"
    const val SERVER_SHUTDOWN = "server_shutdown"
    const val DISPLAY_FAILED = "display_failed"
    const val PERMISSION_MISSING = "permission_missing"
}

/** INPUT_MODE `mode` values (§7.3). */
object InputModeName {
    const val TOUCH = "touch"
    const val TRACKPAD = "trackpad"
}

/** HELLO `network` values (§5). */
object NetworkKind {
    const val METERED = "metered"
    const val UNMETERED = "unmetered"
}

/** SERVER_HELLO `addresses[].kind` values (§1.1). */
object AddressKind {
    const val LAN = "lan"
    const val TAILSCALE = "tailscale"
    const val TAILSCALE_DNS = "tailscale-dns"
}

/** SERVER_HELLO `platform` and TXT `os` values (§1, §4): the host's operating system. */
object HostPlatform {
    const val MACOS = "macos"
    const val WINDOWS = "windows"
    const val LINUX = "linux"
}

/** HELLO `features` values (§5). */
object Feature {
    const val MIC = "mic"
}

/** MIC_STATE `reason` values (§7.2). */
object MicReason {
    const val STOPPED = "stopped"
    const val DISABLED = "disabled"
    const val PERMISSION_DENIED = "permission_denied"
    const val BACKGROUND = "background"
    const val UNAVAILABLE = "unavailable"
}

/** Codec names used in HELLO `codecs` and STREAM_CONFIG `codec`. */
object CodecName {
    const val HEVC = "hevc"
    const val H264 = "h264"
}

/** Any violation of the wire contract. The connection must be closed. */
class ProtocolException(message: String, cause: Throwable? = null) : java.io.IOException(message, cause)
