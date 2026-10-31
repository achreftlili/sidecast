package com.achreftlili.sidecast.protocol

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** docs/PROTOCOL.md §8 test vectors, reproduced exactly. */
class ProtocolVectorsTest {
    private fun hex(s: String) = hexToBytes(s.replace(" ", ""))

    // 8.1 Pairing proof
    @Test
    fun pairingProofVector() {
        val fingerprint = sha256("sidecast-test-cert".toByteArray(Charsets.US_ASCII))
        assertEquals("004ac6c710efca069afd7657c7262b316b18e1595f710ea8a5d2289d04b9d196", fingerprint.toHex())

        val nonce = hex("000102030405060708090a0b0c0d0e0f")
        val proof = PairingCrypto.proof("123456", fingerprint, nonce)
        assertEquals("e34b30e707f974d1fa4e3e5afdc523c3fc9a39ab4c7d43f42af27dcb04244afe", proof.toHex())
        assertEquals("40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=", Base64Codec.encode(proof))
        assertEquals("AAECAwQFBgcICQoLDA0ODw==", Base64Codec.encode(nonce))
    }

    @Test
    fun pairingProofSurvivesPairProofJsonRoundTrip() {
        val fingerprint = sha256("sidecast-test-cert".toByteArray(Charsets.US_ASCII))
        val nonce = hex("000102030405060708090a0b0c0d0e0f")
        val message = PairProof(nonce, PairingCrypto.proof("123456", fingerprint, nonce))
        val payload = String(MessageCodec.encodePayload(message), Charsets.UTF_8)
        // Android's org.json escapes "/" as "\/": both are valid JSON for the same string.
        assertTrue(payload, payload.replace("\\/", "/").contains("\"40sw5wf5dNH6Tj5a/cUjw/yaOatMfUP0KvJ9ywQkSv4=\""))
        assertEquals(message, MessageCodec.decode(MessageType.PAIR_PROOF, MessageCodec.encodePayload(message)))
    }

    // 8.2 Token hash
    @Test
    fun tokenHashVector() {
        val token = ByteArray(32) { 0xAA.toByte() }
        assertEquals(
            "e0e77a507412b120f6ede61f62295b1a7b2ff19d3dcc8f7253e51663470c888e",
            PairingCrypto.tokenHash(token).toHex(),
        )
    }

    // 8.3 Framing
    @Test
    fun frameAckVector() {
        assertArrayEquals(hex("11 00 00 00 04 00 00 01 02"), MessageCodec.encode(FrameAck(258)))
    }

    @Test
    fun touchVector() {
        val touch = Touch(
            eventTimeMs = 1000,
            action = Touch.ACTION_MOVE,
            actionPointerId = 0,
            pointers = listOf(TouchPointer(0, 0.5f, 0.25f)),
        )
        assertArrayEquals(
            hex("10 00 00 00 10  00 00 03 e8  02  00  01  00  3f 00 00 00  3e 80 00 00"),
            MessageCodec.encode(touch),
        )
    }

    @Test
    fun videoFrameHeaderVector() {
        val bytes = hex("85 00 00 00 12  00 00 00 07  01  00 00 00 00 00 0f 42 40  00 00 00 01 40")
        val frame = FrameReader(bytes.inputStream()).next()!!
        assertEquals(MessageType.VIDEO_FRAME, frame.type)
        val video = MessageCodec.decode(frame) as VideoFrame
        assertEquals(7L, video.seq)
        assertTrue(video.isKeyframe)
        assertEquals(1_000_000L, video.ptsMicros)
        assertArrayEquals(hex("00 00 00 01 40"), video.accessUnit())

        // And the encoder produces the same bytes.
        assertArrayEquals(bytes, MessageCodec.encode(VideoFrame(7, Protocol.FLAG_KEYFRAME, 1_000_000, hex("00 00 00 01 40"))))
    }

    @Test
    fun textVector() {
        val bytes = hex("14 00 00 00 02 c3 a9")
        assertArrayEquals(bytes, MessageCodec.encode(Text("é")))
        assertEquals(Text("é"), MessageCodec.decode(FrameReader(bytes.inputStream()).next()!!))
    }

    @Test
    fun keyVectors() {
        assertArrayEquals(hex("15 00 00 00 02 01 00"), MessageCodec.encode(Key(Key.RETURN)))
        assertArrayEquals(hex("15 00 00 00 02 06 01"), MessageCodec.encode(Key(Key.LEFT, Key.MOD_SHIFT)))
        assertEquals(Key(Key.LEFT, Key.MOD_SHIFT), MessageCodec.decode(MessageType.KEY, hex("06 01")))
    }

    @Test
    fun audioVector() {
        val bytes = hex("16 00 00 00 08 00 00 00 01 01 00 fe ff")
        assertArrayEquals(bytes, MessageCodec.encode(Audio(1, shortArrayOf(1, -2))))
        assertEquals(Audio(1, shortArrayOf(1, -2)), MessageCodec.decode(FrameReader(bytes.inputStream()).next()!!))
    }

    @Test
    fun cursorVector() {
        val bytes = hex("89 00 00 00 08 3f 00 00 00 3e 80 00 00")
        assertEquals(Cursor(0.5f, 0.25f), MessageCodec.decode(FrameReader(bytes.inputStream()).next()!!))
        assertArrayEquals(bytes, MessageCodec.encode(Cursor(0.5f, 0.25f)))
    }

    @Test
    fun pingVector() {
        assertArrayEquals(hex("13 00 00 00 08 00 00 00 00 00 00 00 01"), MessageCodec.encode(Ping(1)))
    }

    // 8.4 SERVER_HELLO platform
    @Test
    fun serverHelloPlatformVectors() {
        fun platform(json: String) = (MessageCodec.decode(MessageType.SERVER_HELLO, json.toByteArray()) as ServerHello).platform
        val known = setOf(HostPlatform.MACOS, HostPlatform.WINDOWS, HostPlatform.LINUX)
        assertEquals(HostPlatform.MACOS, platform("""{"protocol":1,"serverId":"S","serverName":"Studio","platform":"macos"}"""))
        assertEquals(HostPlatform.WINDOWS, platform("""{"protocol":1,"serverId":"S","serverName":"Studio","platform":"windows"}"""))
        assertEquals(HostPlatform.LINUX, platform("""{"protocol":1,"serverId":"S","serverName":"Studio","platform":"linux"}"""))
        assertEquals(HostPlatform.MACOS, platform("""{"protocol":1,"serverId":"S","serverName":"Studio"}"""))
        val unknown = platform("""{"protocol":1,"serverId":"S","serverName":"Studio","platform":"beos"}""")
        assertTrue(unknown !in known) // shown as a generic computer
    }
}
