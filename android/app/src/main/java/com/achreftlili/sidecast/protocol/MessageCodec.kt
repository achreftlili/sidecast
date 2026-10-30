package com.achreftlili.sidecast.protocol

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.CharBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import kotlin.math.roundToLong

/**
 * Encoders and decoders for every v1 message, in both directions (the server-side encoders are
 * used by tests and by anyone building a fake Mac). Uses only the org.json subset that exists
 * on Android.
 */
object MessageCodec {
    private const val TOUCH_HEADER = 7 // eventTimeMs u32 + action u8 + actionPointerId u8 + count u8
    private const val TOUCH_POINTER = 9 // id u8 + x f32 + y f32
    private const val VIDEO_HEADER = 13 // seq u32 + flags u8 + pts u64
    private const val AUDIO_HEADER = 4 // seq u32
    private const val CURSOR_SIZE = 8 // x f32 + y f32

    /** Full wire frame (header + payload). */
    fun encode(message: Message): ByteArray = encodeFrame(message.type, encodePayload(message))

    fun encodePayload(message: Message): ByteArray = when (message) {
        is Hello -> json {
            put("protocol", message.protocol)
            put("deviceId", message.deviceId)
            put("deviceName", message.deviceName)
            put(
                "screen",
                JSONObject()
                    .put("width", message.screen.width)
                    .put("height", message.screen.height)
                    .put("densityDpi", message.screen.densityDpi)
                    // Two decimals: avoids float noise such as 59.939998626708984.
                    .put("refreshRate", (message.screen.refreshRate * 100.0).roundToLong() / 100.0),
            )
            val codecs = JSONArray()
            for (c in message.codecs) {
                codecs.put(
                    JSONObject()
                        .put("name", c.name)
                        .put("maxWidth", c.maxWidth)
                        .put("maxHeight", c.maxHeight),
                )
            }
            put("codecs", codecs)
            if (message.features.isNotEmpty()) put("features", JSONArray(message.features))
            message.network?.let { put("network", it) }
            message.limits?.let {
                put("limits", JSONObject().put("maxBitrateKbps", it.maxBitrateKbps).put("maxFps", it.maxFps))
            }
        }
        is Auth -> json {
            put("deviceId", message.deviceId)
            put("token", Base64Codec.encode(message.token))
        }
        PairStart -> ByteArray(0)
        is PairProof -> json {
            put("nonce", Base64Codec.encode(message.nonce))
            put("proof", Base64Codec.encode(message.proof))
        }
        is Touch -> encodeTouch(message)
        is FrameAck -> ByteBuffer.allocate(4).putInt(message.seq.toInt()).array()
        KeyframeRequest -> ByteArray(0)
        is Ping -> ByteBuffer.allocate(8).putLong(message.t).array()
        is Text -> encodeText(message)
        is Audio -> encodeAudio(message)
        is InputMode -> json { put("mode", message.mode) }
        is MicState -> json {
            put("capturing", message.capturing)
            message.reason?.let { put("reason", it) }
        }
        is Cursor -> ByteBuffer.allocate(CURSOR_SIZE).putFloat(message.x).putFloat(message.y).array()
        is MicControl -> json {
            put("active", message.active)
            message.sampleRate?.let { put("sampleRate", it) }
            message.channels?.let { put("channels", it) }
        }
        is Key -> {
            require(message.key in 0..0xFF && message.modifiers in 0..0xFF) { "KEY fields are u8" }
            byteArrayOf(message.key.toByte(), message.modifiers.toByte())
        }
        is ServerHello -> json {
            put("protocol", message.protocol)
            put("serverId", message.serverId)
            put("serverName", message.serverName)
            message.port?.let { put("port", it) }
            message.addresses?.let { list ->
                val a = JSONArray()
                for (address in list) a.put(JSONObject().put("host", address.host).put("kind", address.kind))
                put("addresses", a)
            }
        }
        is PairChallenge -> json {
            put("expiresInSec", message.expiresInSec)
            put("attemptsLeft", message.attemptsLeft)
        }
        is AuthResult -> json {
            put("ok", message.ok)
            message.token?.let { put("token", Base64Codec.encode(it)) }
            message.reason?.let { put("reason", it) }
            message.attemptsLeft?.let { put("attemptsLeft", it) }
        }
        is StreamConfig -> json {
            put("codec", message.codec)
            put("width", message.width)
            put("height", message.height)
            put("fps", message.fps)
        }
        is VideoFrame -> ByteBuffer.allocate(VIDEO_HEADER + message.dataLength)
            .putInt(message.seq.toInt())
            .put(message.flags.toByte())
            .putLong(message.ptsMicros)
            .put(message.data, message.dataOffset, message.dataLength)
            .array()
        is Pong -> ByteBuffer.allocate(8).putLong(message.t).array()
        is Bye -> json { put("reason", message.reason) }
    }

    private fun encodeTouch(touch: Touch): ByteArray {
        val count = touch.pointers.size
        require(count in 1..Protocol.MAX_TOUCH_POINTERS) { "TOUCH needs 1..10 pointers, got $count" }
        val buf = ByteBuffer.allocate(TOUCH_HEADER + count * TOUCH_POINTER)
        buf.putInt(touch.eventTimeMs.toInt()) // u32, wraps
        buf.put(touch.action.toByte())
        buf.put(touch.actionPointerId.toByte())
        buf.put(count.toByte())
        for (p in touch.pointers) {
            buf.put(p.id.toByte())
            buf.putFloat(p.x)
            buf.putFloat(p.y)
        }
        return buf.array()
    }

    private fun encodeAudio(audio: Audio): ByteArray {
        val count = audio.samples.size
        require(count in 1..Protocol.MAX_AUDIO_SAMPLES) { "AUDIO needs 1..${Protocol.MAX_AUDIO_SAMPLES} samples, got $count" }
        val out = ByteArray(AUDIO_HEADER + count * 2)
        val seq = audio.seq.toInt() // u32, wraps
        out[0] = (seq ushr 24).toByte()
        out[1] = (seq ushr 16).toByte()
        out[2] = (seq ushr 8).toByte()
        out[3] = seq.toByte()
        var o = AUDIO_HEADER
        for (sample in audio.samples) {
            val v = sample.toInt()
            out[o++] = v.toByte() // little-endian
            out[o++] = (v shr 8).toByte()
        }
        return out
    }

    private fun encodeText(text: Text): ByteArray {
        val bytes = try {
            Charsets.UTF_8.newEncoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
                .encode(CharBuffer.wrap(text.text))
                .let { buf -> ByteArray(buf.remaining()).also(buf::get) }
        } catch (e: CharacterCodingException) {
            throw IllegalArgumentException("TEXT is not valid UTF-16 (unpaired surrogate)", e)
        }
        require(bytes.size in Protocol.MIN_TEXT_BYTES..Protocol.MAX_TEXT_BYTES) {
            "TEXT needs ${Protocol.MIN_TEXT_BYTES}..${Protocol.MAX_TEXT_BYTES} UTF-8 bytes, got ${bytes.size} (use Text.split)"
        }
        return bytes
    }

    fun decode(frame: Frame): Message? = decode(frame.type, frame.payload)

    /**
     * Decodes one payload. Returns null for an unknown [type], and for a CURSOR of the wrong size
     * (§7.3: ignored, never fatal).
     * @throws ProtocolException on any other malformed payload.
     */
    fun decode(type: Int, payload: ByteArray): Message? = try {
        decodeOrThrow(type, payload)
    } catch (e: JSONException) {
        throw ProtocolException("malformed JSON in type 0x%02x: %s".format(type, e.message), e)
    } catch (e: IllegalArgumentException) {
        throw ProtocolException("malformed payload in type 0x%02x: %s".format(type, e.message), e)
    }

    private fun decodeOrThrow(type: Int, payload: ByteArray): Message? = when (type) {
        MessageType.HELLO -> payload.json().run {
            val screen = getJSONObject("screen")
            val codecsJson = optJSONArray("codecs") ?: JSONArray()
            Hello(
                protocol = getInt("protocol"),
                deviceId = requireString("deviceId"),
                deviceName = requireString("deviceName"),
                screen = ScreenInfo(
                    width = screen.getInt("width"),
                    height = screen.getInt("height"),
                    densityDpi = screen.getInt("densityDpi"),
                    refreshRate = screen.getDouble("refreshRate").toFloat(),
                ),
                codecs = List(codecsJson.length()) { i ->
                    val c = codecsJson.getJSONObject(i)
                    CodecSupport(c.requireString("name"), c.getInt("maxWidth"), c.getInt("maxHeight"))
                },
                features = optJSONArray("features")?.let { a -> List(a.length()) { a.getString(it) } } ?: emptyList(),
                network = optStringOrNull("network"),
                limits = optJSONObject("limits")?.let { StreamLimits(it.getInt("maxBitrateKbps"), it.getInt("maxFps")) },
            )
        }
        MessageType.AUTH -> payload.json().run {
            Auth(requireString("deviceId"), Base64Codec.decode(requireString("token")))
        }
        MessageType.PAIR_START -> PairStart
        MessageType.PAIR_PROOF -> payload.json().run {
            val nonce = Base64Codec.decode(requireString("nonce"))
            val proof = Base64Codec.decode(requireString("proof"))
            if (nonce.size != Protocol.NONCE_SIZE || proof.size != Protocol.PROOF_SIZE) {
                throw ProtocolException("PAIR_PROOF nonce/proof have wrong sizes")
            }
            PairProof(nonce, proof)
        }
        MessageType.TOUCH -> decodeTouch(payload)
        MessageType.FRAME_ACK -> {
            requireSize(type, payload, 4)
            FrameAck(ByteBuffer.wrap(payload).int.toLong() and 0xFFFF_FFFFL)
        }
        MessageType.KEYFRAME_REQUEST -> KeyframeRequest
        MessageType.PING -> {
            requireSize(type, payload, 8)
            Ping(ByteBuffer.wrap(payload).long)
        }
        MessageType.TEXT -> {
            if (payload.size !in Protocol.MIN_TEXT_BYTES..Protocol.MAX_TEXT_BYTES) {
                throw ProtocolException("TEXT payload of ${payload.size} bytes")
            }
            val text = try {
                Charsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(payload))
                    .toString()
            } catch (e: CharacterCodingException) {
                throw ProtocolException("TEXT is not valid UTF-8", e)
            }
            Text(text)
        }
        MessageType.AUDIO -> decodeAudio(payload)
        MessageType.INPUT_MODE -> payload.json().run { InputMode(requireString("mode")) }
        MessageType.MIC_STATE -> payload.json().run {
            MicState(getBoolean("capturing"), optStringOrNull("reason"))
        }
        MessageType.CURSOR -> if (payload.size == CURSOR_SIZE) {
            ByteBuffer.wrap(payload).let { Cursor(it.float, it.float) }
        } else {
            null
        }
        MessageType.MIC_CONTROL -> payload.json().run {
            MicControl(
                active = getBoolean("active"),
                sampleRate = if (has("sampleRate") && !isNull("sampleRate")) getInt("sampleRate") else null,
                channels = if (has("channels") && !isNull("channels")) getInt("channels") else null,
            )
        }
        MessageType.KEY -> {
            requireSize(type, payload, 2)
            Key(payload[0].toInt() and 0xFF, payload[1].toInt() and 0xFF)
        }
        MessageType.SERVER_HELLO -> payload.json().run {
            ServerHello(
                protocol = getInt("protocol"),
                serverId = requireString("serverId"),
                serverName = optStringOrNull("serverName") ?: "",
                // Optional and tolerant: a bad port or entry is skipped, never fatal.
                port = (opt("port") as? Number)?.toInt()?.takeIf { it in 1..65535 },
                addresses = optJSONArray("addresses")?.let(::decodeAddresses),
            )
        }
        MessageType.PAIR_CHALLENGE -> payload.json().run {
            PairChallenge(getInt("expiresInSec"), getInt("attemptsLeft"))
        }
        MessageType.AUTH_RESULT -> payload.json().run {
            AuthResult(
                ok = getBoolean("ok"),
                token = optStringOrNull("token")?.let(Base64Codec::decode),
                reason = optStringOrNull("reason"),
                attemptsLeft = if (has("attemptsLeft") && !isNull("attemptsLeft")) getInt("attemptsLeft") else null,
            )
        }
        MessageType.STREAM_CONFIG -> payload.json().run {
            StreamConfig(requireString("codec"), getInt("width"), getInt("height"), getInt("fps"))
        }
        MessageType.VIDEO_FRAME -> {
            if (payload.size < VIDEO_HEADER) {
                throw ProtocolException("VIDEO_FRAME shorter than its 13-byte header")
            }
            val buf = ByteBuffer.wrap(payload)
            val seq = buf.int.toLong() and 0xFFFF_FFFFL
            val flags = buf.get().toInt() and 0xFF
            val pts = buf.long
            VideoFrame(seq, flags, pts, payload, VIDEO_HEADER, payload.size - VIDEO_HEADER)
        }
        MessageType.PONG -> {
            requireSize(type, payload, 8)
            Pong(ByteBuffer.wrap(payload).long)
        }
        MessageType.BYE -> payload.json().run { Bye(optStringOrNull("reason") ?: "") }
        else -> null
    }

    private fun decodeAddresses(array: JSONArray): List<ServerAddress> = (0 until array.length()).mapNotNull { i ->
        val o = array.optJSONObject(i) ?: return@mapNotNull null
        val host = (o.opt("host") as? String)?.trim()?.takeIf { it.isNotEmpty() } ?: return@mapNotNull null
        val kind = o.opt("kind") as? String ?: return@mapNotNull null
        ServerAddress(host, kind)
    }

    private fun decodeTouch(payload: ByteArray): Touch {
        if (payload.size < TOUCH_HEADER) throw ProtocolException("TOUCH shorter than its header")
        val buf = ByteBuffer.wrap(payload)
        val time = buf.int.toLong() and 0xFFFF_FFFFL
        val action = buf.get().toInt() and 0xFF
        val actionPointerId = buf.get().toInt() and 0xFF
        val count = buf.get().toInt() and 0xFF
        if (count !in 1..Protocol.MAX_TOUCH_POINTERS) throw ProtocolException("TOUCH count $count out of range")
        if (payload.size < TOUCH_HEADER + count * TOUCH_POINTER) throw ProtocolException("TOUCH truncated")
        val pointers = List(count) {
            TouchPointer(buf.get().toInt() and 0xFF, buf.float, buf.float)
        }
        return Touch(time, action, actionPointerId, pointers)
    }

    private fun decodeAudio(payload: ByteArray): Audio {
        val bytes = payload.size - AUDIO_HEADER
        if (bytes <= 0 || bytes % 2 != 0 || bytes / 2 > Protocol.MAX_AUDIO_SAMPLES) {
            throw ProtocolException("AUDIO payload of ${payload.size} bytes")
        }
        val buf = ByteBuffer.wrap(payload)
        val seq = buf.int.toLong() and 0xFFFF_FFFFL
        buf.order(ByteOrder.LITTLE_ENDIAN)
        val samples = ShortArray(bytes / 2) { buf.short }
        return Audio(seq, samples)
    }

    private fun requireSize(type: Int, payload: ByteArray, size: Int) {
        if (payload.size != size) {
            throw ProtocolException("type 0x%02x needs %d bytes, got %d".format(type, size, payload.size))
        }
    }

    private inline fun json(build: JSONObject.() -> Unit): ByteArray =
        JSONObject().apply(build).toString().toByteArray(Charsets.UTF_8)

    private fun ByteArray.json(): JSONObject = JSONObject(String(this, Charsets.UTF_8))

    private fun JSONObject.optStringOrNull(key: String): String? =
        if (has(key) && !isNull(key)) getString(key) else null

    private fun JSONObject.requireString(key: String): String =
        optStringOrNull(key) ?: throw ProtocolException("missing \"$key\"")
}
