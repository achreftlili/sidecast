package com.achreftlili.sidecast.protocol

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class MessageCodecTest {
    private fun roundTrip(message: Message): Message {
        val frame = FrameReader(MessageCodec.encode(message).inputStream()).next()!!
        assertEquals(message.type, frame.type)
        return MessageCodec.decode(frame)!!
    }

    private fun json(message: Message) = JSONObject(String(MessageCodec.encodePayload(message), Charsets.UTF_8))

    private val hello = Hello(
        deviceId = "8c1f2d3e-0000-4000-8000-000000000001",
        deviceName = "Pixel 8",
        screen = ScreenInfo(2400, 1080, 420, 120f),
        codecs = listOf(CodecSupport("hevc", 3840, 2160), CodecSupport("h264", 1920, 1080)),
    )

    @Test
    fun jsonMessagesRoundTrip() {
        val messages = listOf(
            hello,
            Auth("device-1", ByteArray(32) { it.toByte() }),
            PairProof(ByteArray(16) { 1 }, ByteArray(32) { 2 }),
            ServerHello(1, "6a1b8a8e-1111-4222-8333-444455556666", "Achref’s MacBook Pro"),
            ServerHello(
                1,
                "srv",
                "Studio",
                port = 47800,
                addresses = listOf(ServerAddress("192.168.1.20", AddressKind.LAN), ServerAddress("100.101.102.103", AddressKind.TAILSCALE)),
            ),
            ServerHello(1, "srv", "Studio", addresses = emptyList()),
            hello.copy(network = NetworkKind.METERED, limits = StreamLimits(4000, 30)),
            hello.copy(network = NetworkKind.UNMETERED),
            PairChallenge(expiresInSec = 120, attemptsLeft = 5),
            AuthResult(ok = true, token = ByteArray(32) { 9 }),
            AuthResult(ok = true),
            AuthResult(ok = false, reason = Reason.BAD_CODE, attemptsLeft = 3),
            AuthResult(ok = false, reason = Reason.UNKNOWN_DEVICE),
            StreamConfig("hevc", 2400, 1080, 60),
            Bye(Reason.PERMISSION_MISSING),
            hello.copy(features = listOf(Feature.MIC)),
            MicState(capturing = true),
            MicState(capturing = false, reason = MicReason.PERMISSION_DENIED),
            MicControl(active = true, sampleRate = 48_000, channels = 1),
            MicControl(active = false),
            InputMode(InputModeName.TOUCH),
            InputMode(InputModeName.TRACKPAD),
        )
        for (m in messages) assertEquals(m, roundTrip(m))
    }

    @Test
    fun binaryMessagesRoundTrip() {
        val pointers = List(10) { TouchPointer(it, it / 10f, 1f - it / 10f) }
        val messages = listOf(
            PairStart,
            KeyframeRequest,
            Touch(0xFFFF_FFFFL, Touch.ACTION_POINTER_DOWN, 9, pointers),
            Touch(5, Touch.ACTION_UP, 3, listOf(TouchPointer(3, -0.01f, 1.02f))),
            FrameAck(0xFFFF_FFFFL),
            FrameAck(0),
            Ping(Long.MAX_VALUE),
            Pong(123_456_789L),
            VideoFrame(0xFFFF_FFFEL, 0, 42, byteArrayOf(0, 0, 0, 1, 0x26, 1)),
            VideoFrame(3, Protocol.FLAG_KEYFRAME, 0, ByteArray(0)),
            Text("a"),
            Text("héllo wörld 👋🏽 日本語"),
            Text("x".repeat(Protocol.MAX_TEXT_BYTES)),
            Key(Key.BACKSPACE),
            Key(Key.PAGE_DOWN, Key.MOD_SHIFT or Key.MOD_CONTROL or Key.MOD_OPTION or Key.MOD_COMMAND),
            Key(0xFF, 0xFF),
            Audio(0, shortArrayOf(0)),
            Audio(0xFFFF_FFFFL, ShortArray(Protocol.MAX_AUDIO_SAMPLES) { (it * 13 - 32768).toShort() }),
            Audio(7, shortArrayOf(Short.MIN_VALUE, -1, 0, 1, Short.MAX_VALUE)),
        )
        for (m in messages) assertEquals(m, roundTrip(m))
    }

    @Test
    fun helloFeaturesAreOptional() {
        assertFalse(json(hello).has("features")) // absent = none (§5)
        val o = json(hello.copy(features = listOf(Feature.MIC)))
        assertEquals("mic", o.getJSONArray("features").getString(0))
        val legacy = """{"protocol":1,"deviceId":"d","deviceName":"n","screen":{"width":1,"height":1,"densityDpi":1,"refreshRate":60}}"""
        assertEquals(emptyList<String>(), (MessageCodec.decode(MessageType.HELLO, legacy.toByteArray()) as Hello).features)
    }

    @Test
    fun serverHelloAddressesAreOptionalAndTolerant() {
        fun decode(json: String) = MessageCodec.decode(MessageType.SERVER_HELLO, json.toByteArray()) as ServerHello
        val legacy = decode("""{"protocol":1,"serverId":"s","serverName":"Mac"}""")
        assertNull(legacy.port)
        assertNull(legacy.addresses) // "not said", as opposed to an empty list
        val messy = decode(
            """{"protocol":1,"serverId":"s","serverName":"Mac","port":70000,"addresses":[
                {"host":"192.168.1.20","kind":"lan"}, {"host":"","kind":"lan"}, {"kind":"tailscale"},
                {"host":"mac.tail1234.ts.net","kind":"tailscale-dns","extra":1}, "junk", {"host":" 10.0.0.5 ","kind":"lan"},
                {"host":"203.0.113.9","kind":"public"}]}""",
        )
        assertNull("port out of range is ignored", messy.port)
        assertEquals(
            listOf(
                ServerAddress("192.168.1.20", "lan"),
                ServerAddress("mac.tail1234.ts.net", "tailscale-dns"),
                ServerAddress("10.0.0.5", "lan"),
                ServerAddress("203.0.113.9", "public"), // unknown kinds are kept; the planner ignores them
            ),
            messy.addresses,
        )
        assertNull(decode("""{"protocol":1,"serverId":"s","addresses":"nope","port":"x"}""").addresses)
    }

    @Test
    fun helloNetworkAndLimitsAreOptional() {
        val o = json(hello)
        assertFalse(o.has("network"))
        assertFalse(o.has("limits"))
        val saver = json(hello.copy(network = NetworkKind.METERED, limits = StreamLimits(4000, 30)))
        assertEquals("metered", saver.getString("network"))
        assertEquals(4000, saver.getJSONObject("limits").getInt("maxBitrateKbps"))
        assertEquals(30, saver.getJSONObject("limits").getInt("maxFps"))
    }

    @Test
    fun cursorOfTheWrongSizeIsIgnoredNotFatal() {
        assertEquals(0x89, MessageType.CURSOR)
        assertTrue(MessageType.isKnown(MessageType.CURSOR))
        for (size in listOf(0, 4, 7, 9, 16)) assertNull("size $size", MessageCodec.decode(MessageType.CURSOR, ByteArray(size)))
    }

    @Test
    fun cursorOnDisplayRules() {
        assertTrue(Cursor(0f, 0f).onDisplay)
        assertTrue(Cursor(1f, 1f).onDisplay)
        assertTrue(Cursor(0.5f, 0.25f).onDisplay)
        assertFalse(Cursor(-1f, -1f).onDisplay) // "not on this display"
        assertFalse(Cursor(1.01f, 0.5f).onDisplay)
        assertFalse(Cursor(0.5f, -0.01f).onDisplay)
        assertFalse(Cursor(Float.NaN, 0.5f).onDisplay)
    }

    @Test
    fun inputModeMatchesSpecShape() {
        assertEquals(0x18, MessageType.INPUT_MODE)
        assertTrue(MessageType.isKnown(MessageType.INPUT_MODE))
        assertEquals("{\"mode\":\"trackpad\"}", String(MessageCodec.encodePayload(InputMode(InputModeName.TRACKPAD))))
        val frame = MessageCodec.encode(InputMode(InputModeName.TOUCH))
        assertEquals(0x18, frame[0].toInt())
        assertEquals("{\"mode\":\"touch\"}", String(frame, 5, frame.size - 5))
        // A future mode still decodes (the server ignores unknown modes).
        assertEquals(InputMode("pen"), MessageCodec.decode(MessageType.INPUT_MODE, "{\"mode\":\"pen\"}".toByteArray()))
    }

    @Test
    fun micJsonMatchesSpecShape() {
        assertEquals("{\"capturing\":true}", String(MessageCodec.encodePayload(MicState(true))))
        val stopped = json(MicState(false, MicReason.STOPPED))
        assertFalse(stopped.getBoolean("capturing"))
        assertEquals("stopped", stopped.getString("reason"))
        val control = MessageCodec.decode(MessageType.MIC_CONTROL, "{\"active\":true,\"sampleRate\":48000,\"channels\":1,\"x\":0}".toByteArray())
        assertEquals(MicControl(true, 48_000, 1), control)
        assertEquals(MicControl(false), MessageCodec.decode(MessageType.MIC_CONTROL, "{\"active\":false}".toByteArray()))
        assertEquals(0x16, MessageType.AUDIO)
        assertEquals(0x17, MessageType.MIC_STATE)
        assertEquals(0x88, MessageType.MIC_CONTROL)
    }

    @Test
    fun audioSamplesAreLittleEndianAfterABigEndianSeq() {
        val payload = MessageCodec.encodePayload(Audio(0x01020304, shortArrayOf(0x1234, -0x1235)))
        assertEquals(listOf(1, 2, 3, 4, 0x34, 0x12, 0xCB, 0xED), payload.map { it.toInt() and 0xFF })
    }

    @Test
    fun audioEncoderEnforcesTheSampleCount() {
        for (n in listOf(0, Protocol.MAX_AUDIO_SAMPLES + 1)) {
            try {
                MessageCodec.encode(Audio(0, ShortArray(n)))
                fail("encoded AUDIO with $n samples")
            } catch (_: IllegalArgumentException) {
            }
        }
    }

    @Test
    fun keyboardTypesAreKnownClientTypes() {
        assertEquals(0x14, MessageType.TEXT)
        assertEquals(0x15, MessageType.KEY)
        assertTrue(MessageType.isKnown(MessageType.TEXT))
        assertTrue(MessageType.isKnown(MessageType.KEY))
        assertTrue(MessageCodec.decode(MessageType.TEXT, "hi".toByteArray()) is ClientMessage)
    }

    @Test
    fun textEncoderEnforcesTheContract() {
        for (bad in listOf("", "x".repeat(Protocol.MAX_TEXT_BYTES + 1), "é".repeat(2049), "a\uD800b")) {
            try {
                MessageCodec.encode(Text(bad))
                fail("encoded invalid TEXT of ${bad.length} chars")
            } catch (_: IllegalArgumentException) {
            }
        }
    }

    @Test
    fun malformedKeyboardPayloadsAreProtocolErrors() {
        val cases = listOf(
            MessageType.TEXT to ByteArray(0),
            MessageType.TEXT to ByteArray(Protocol.MAX_TEXT_BYTES + 1) { 'a'.code.toByte() },
            MessageType.TEXT to byteArrayOf(0xC3.toByte()), // truncated sequence
            MessageType.TEXT to byteArrayOf(0xED.toByte(), 0xA0.toByte(), 0x80.toByte()), // encoded surrogate
            MessageType.KEY to byteArrayOf(1),
            MessageType.KEY to byteArrayOf(1, 0, 0),
            MessageType.AUDIO to byteArrayOf(0, 0, 0, 1), // no samples
            MessageType.AUDIO to byteArrayOf(0, 0, 0, 1, 5), // odd byte count
            MessageType.AUDIO to ByteArray(4 + 2 * (Protocol.MAX_AUDIO_SAMPLES + 1)),
            MessageType.AUDIO to byteArrayOf(0, 0),
        )
        for ((type, payload) in cases) {
            try {
                MessageCodec.decode(type, payload)
                fail("decoded malformed type $type of ${payload.size} bytes")
            } catch (_: ProtocolException) {
            }
        }
    }

    @Test
    fun helloJsonMatchesSpecShape() {
        val o = json(hello)
        assertEquals(1, o.getInt("protocol"))
        assertEquals("Pixel 8", o.getString("deviceName"))
        val screen = o.getJSONObject("screen")
        assertEquals(2400, screen.getInt("width"))
        assertEquals(1080, screen.getInt("height"))
        assertEquals(420, screen.getInt("densityDpi"))
        assertEquals(120.0, screen.getDouble("refreshRate"), 0.0)
        val codecs = o.getJSONArray("codecs")
        assertEquals("hevc", codecs.getJSONObject(0).getString("name"))
        assertEquals(3840, codecs.getJSONObject(0).getInt("maxWidth"))
        assertEquals(1080, codecs.getJSONObject(1).getInt("maxHeight"))
    }

    @Test
    fun refreshRateHasNoFloatNoise() {
        val o = json(hello.copy(screen = hello.screen.copy(refreshRate = 59.94f)))
        assertEquals("59.94", o.getJSONObject("screen").get("refreshRate").toString())
    }

    @Test
    fun authResultOmitsAbsentOptionalFields() {
        val o = json(AuthResult(ok = true))
        assertEquals(setOf("ok"), o.keys().asSequence().toSet())
    }

    @Test
    fun ignoresUnknownJsonKeys() {
        val payload = """{"protocol":1,"serverId":"abc","serverName":"Mac","future":{"x":1},"list":[1,2]}"""
        assertEquals(ServerHello(1, "abc", "Mac"), MessageCodec.decode(MessageType.SERVER_HELLO, payload.toByteArray()))
        val cfg = """{"codec":"h264","width":1920,"height":1080,"fps":30,"bitrate":20000000}"""
        assertEquals(StreamConfig("h264", 1920, 1080, 30), MessageCodec.decode(MessageType.STREAM_CONFIG, cfg.toByteArray()))
    }

    @Test
    fun acceptsUnpaddedBase64AndEscapedSlashes() {
        val payload = """{"ok":true,"token":"40sw5wf5dNH6Tj5a\/cUjw\/yaOatMfUP0KvJ9ywQkSv4"}"""
        val result = MessageCodec.decode(MessageType.AUTH_RESULT, payload.toByteArray()) as AuthResult
        assertEquals("e34b30e707f974d1fa4e3e5afdc523c3fc9a39ab4c7d43f42af27dcb04244afe", result.token!!.toHex())
    }

    @Test
    fun unknownTypeDecodesToNull() {
        assertNull(MessageCodec.decode(0x99, ByteArray(3)))
    }

    @Test
    fun malformedPayloadsAreProtocolErrors() {
        val cases = listOf(
            MessageType.SERVER_HELLO to "not json",
            MessageType.SERVER_HELLO to """{"protocol":1}""",
            MessageType.AUTH_RESULT to """{"token":"AAAA"}""",
            MessageType.AUTH_RESULT to """{"ok":true,"token":"%%%"}""",
            MessageType.STREAM_CONFIG to """{"codec":"hevc","width":"wide","height":1,"fps":1}""",
            MessageType.PAIR_PROOF to """{"nonce":"AAAA","proof":"AAAA"}""",
        )
        for ((type, text) in cases) {
            try {
                MessageCodec.decode(type, text.toByteArray())
                fail("expected ProtocolException for $text")
            } catch (_: ProtocolException) {
            }
        }
        val binary = listOf(
            MessageType.FRAME_ACK to ByteArray(3),
            MessageType.PING to ByteArray(9),
            MessageType.PONG to ByteArray(7),
            MessageType.VIDEO_FRAME to ByteArray(12),
            MessageType.TOUCH to byteArrayOf(0, 0, 0, 1, 2, 0, 0), // count 0
            MessageType.TOUCH to byteArrayOf(0, 0, 0, 1, 2, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0), // truncated
        )
        for ((type, bytes) in binary) {
            try {
                MessageCodec.decode(type, bytes)
                fail("expected ProtocolException for type $type")
            } catch (_: ProtocolException) {
            }
        }
    }

    @Test
    fun touchEncoderEnforcesPointerLimits() {
        for (count in listOf(0, 11)) {
            try {
                MessageCodec.encode(Touch(0, Touch.ACTION_MOVE, 0, List(count) { TouchPointer(it, 0f, 0f) }))
                fail("expected failure for $count pointers")
            } catch (_: IllegalArgumentException) {
            }
        }
    }

    @Test
    fun touchEventTimeWraps() {
        val bytes = MessageCodec.encodePayload(Touch(0x1_0000_0005L, Touch.ACTION_DOWN, 0, listOf(TouchPointer(0, 0f, 0f))))
        assertEquals(5L, (MessageCodec.decode(MessageType.TOUCH, bytes) as Touch).eventTimeMs)
    }

    @Test
    fun pairingCodeValidation() {
        assertTrue(PairingCrypto.isValidCode("000000"))
        assertTrue(PairingCrypto.isValidCode("999999"))
        for (bad in listOf("", "12345", "1234567", "12a456", "１２３４５６", " 12345")) {
            assertTrue(bad, !PairingCrypto.isValidCode(bad))
        }
    }
}
