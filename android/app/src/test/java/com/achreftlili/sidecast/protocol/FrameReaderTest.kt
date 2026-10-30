package com.achreftlili.sidecast.protocol

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.EOFException
import java.io.InputStream
import java.io.SequenceInputStream

class FrameReaderTest {
    /** Returns at most [chunk] bytes per read, like a slow socket. */
    private class TrickleStream(bytes: ByteArray, private val chunk: Int) : InputStream() {
        private val inner = ByteArrayInputStream(bytes)
        override fun read(): Int = inner.read()
        override fun read(b: ByteArray, off: Int, len: Int): Int = inner.read(b, off, minOf(len, chunk))
    }

    /** Fails the test if anything tries to read past the header of an oversize frame. */
    private class HeaderOnlyStream(private val header: ByteArray) : InputStream() {
        private var pos = 0
        override fun read(): Int = if (pos < header.size) header[pos++].toInt() and 0xFF else error("read past header")
    }

    private fun frames(vararg messages: Message) =
        messages.map(MessageCodec::encode).fold(ByteArray(0)) { acc, b -> acc + b }

    @Test
    fun readsFramesDeliveredOneByteAtATime() {
        val bytes = frames(Ping(42), FrameAck(7), KeyframeRequest, Bye("server_shutdown"))
        val reader = FrameReader(TrickleStream(bytes, 1))
        assertEquals(Ping(42), MessageCodec.decode(reader.next()!!))
        assertEquals(FrameAck(7), MessageCodec.decode(reader.next()!!))
        assertEquals(KeyframeRequest, MessageCodec.decode(reader.next()!!))
        assertEquals(Bye("server_shutdown"), MessageCodec.decode(reader.next()!!))
        assertNull(reader.next())
    }

    @Test
    fun readsFramesSplitAcrossArbitraryChunks() {
        val video = VideoFrame(1, 0, 99, ByteArray(70_000) { it.toByte() })
        val bytes = frames(video, Pong(5))
        for (chunk in listOf(2, 3, 7, 4096)) {
            val reader = FrameReader(TrickleStream(bytes, chunk))
            assertEquals(video, MessageCodec.decode(reader.next()!!))
            assertEquals(Pong(5), MessageCodec.decode(reader.next()!!))
            assertNull(reader.next())
        }
    }

    @Test
    fun zeroLengthPayload() {
        val frame = FrameReader(ByteArrayInputStream(byteArrayOf(0x12, 0, 0, 0, 0))).next()!!
        assertEquals(MessageType.KEYFRAME_REQUEST, frame.type)
        assertEquals(0, frame.payload.size)
    }

    @Test
    fun skipsUnknownTypes() {
        val unknown = encodeFrame(0x99, ByteArray(10_000) { 1 })
        val unknownEmpty = encodeFrame(0x7F, ByteArray(0))
        val bytes = unknown + MessageCodec.encode(Ping(1)) + unknownEmpty + MessageCodec.encode(Pong(2))
        val reader = FrameReader(TrickleStream(bytes, 333))
        assertEquals(Ping(1), MessageCodec.decode(reader.next()!!))
        assertEquals(Pong(2), MessageCodec.decode(reader.next()!!))
        assertNull(reader.next())
        assertEquals(2L, reader.skippedFrames)
    }

    @Test
    fun acceptsExactly16MiB() {
        val payload = ByteArray(Protocol.MAX_PAYLOAD)
        val reader = FrameReader(ByteArrayInputStream(encodeFrame(MessageType.VIDEO_FRAME, payload)))
        assertEquals(Protocol.MAX_PAYLOAD, reader.next()!!.payload.size)
    }

    @Test
    fun rejectsOversizeBeforeReadingPayload() {
        val tooBig = Protocol.MAX_PAYLOAD + 1
        val header = byteArrayOf(0x85.toByte(), (tooBig ushr 24).toByte(), (tooBig ushr 16).toByte(), (tooBig ushr 8).toByte(), tooBig.toByte())
        try {
            FrameReader(HeaderOnlyStream(header)).next()
            fail("expected ProtocolException")
        } catch (e: ProtocolException) {
            assertTrue(e.message!!.contains("exceeds"))
        }
    }

    @Test
    fun rejectsOversizeUnknownTypeToo() {
        val header = byteArrayOf(0x55, 0xFF.toByte(), 0xFF.toByte(), 0xFF.toByte(), 0xFF.toByte())
        try {
            FrameReader(HeaderOnlyStream(header)).next()
            fail("expected ProtocolException")
        } catch (_: ProtocolException) {
        }
    }

    @Test(expected = EOFException::class)
    fun eofInsideHeader() {
        FrameReader(ByteArrayInputStream(byteArrayOf(0x13, 0, 0))).next()
    }

    @Test(expected = EOFException::class)
    fun eofInsidePayload() {
        val full = MessageCodec.encode(Ping(1))
        FrameReader(ByteArrayInputStream(full.copyOf(full.size - 1))).next()
    }

    @Test
    fun cleanEofAtBoundaryReturnsNull() {
        val reader = FrameReader(SequenceInputStream(ByteArrayInputStream(MessageCodec.encode(Ping(3))), ByteArrayInputStream(ByteArray(0))))
        assertEquals(Ping(3), MessageCodec.decode(reader.next()!!))
        assertNull(reader.next())
    }

    @Test
    fun encodeFrameWritesBigEndianLength() {
        assertArrayEquals(byteArrayOf(0x84.toByte(), 0, 0, 1, 0) + ByteArray(256), encodeFrame(0x84, ByteArray(256)))
    }
}
