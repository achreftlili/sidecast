package com.achreftlili.sidecast.protocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** TEXT splitting (§7.1): 1…4096 UTF-8 bytes per message, cut only between code points. */
class TextSplitTest {
    private val max = Protocol.MAX_TEXT_BYTES

    private fun utf8(s: String) = s.toByteArray(Charsets.UTF_8).size

    /** Every chunk is a valid TEXT payload and the chunks concatenate back to [expected]. */
    private fun assertValidSplit(chunks: List<Text>, expected: String) {
        assertEquals(expected, chunks.joinToString("") { it.text })
        for (c in chunks) {
            assertTrue("chunk of ${utf8(c.text)} bytes", utf8(c.text) in 1..max)
            MessageCodec.encode(c) // strict encoder: throws on a split surrogate pair
        }
    }

    @Test
    fun emptyTextGivesNoMessages() {
        assertEquals(emptyList<Text>(), Text.split(""))
    }

    @Test
    fun shortTextIsOneMessage() {
        assertEquals(listOf(Text("é")), Text.split("é"))
        assertEquals(listOf(Text("x".repeat(max))), Text.split("x".repeat(max)))
    }

    @Test
    fun asciiOverTheLimitSplitsAtExactlyMaxBytes() {
        val text = "x".repeat(max * 2 + 1)
        val chunks = Text.split(text)
        assertEquals(listOf(max, max, 1), chunks.map { utf8(it.text) })
        assertValidSplit(chunks, text)
    }

    @Test
    fun twoByteCharacterNeverStraddlesTheBoundary() {
        val text = "a".repeat(max - 1) + "é" + "b"
        val chunks = Text.split(text)
        assertEquals(listOf("a".repeat(max - 1), "éb"), chunks.map { it.text })
        assertValidSplit(chunks, text)
    }

    @Test
    fun multiByteTextSplitsOnCharacterBoundaries() {
        val text = "é".repeat(max / 2 + 1) // 4098 bytes
        val chunks = Text.split(text)
        assertEquals(listOf(max, 2), chunks.map { utf8(it.text) })
        assertValidSplit(chunks, text)

        val euro = "€".repeat(1366) // 3 bytes each: 4098 bytes; 1365 fit in 4095
        val euroChunks = Text.split(euro)
        assertEquals(listOf(1365 * 3, 3), euroChunks.map { utf8(it.text) })
        assertValidSplit(euroChunks, euro)
    }

    @Test
    fun emojiAtTheBoundaryIsKeptWhole() {
        val emoji = "😀" // U+1F600: surrogate pair, 4 UTF-8 bytes
        val fits = "a".repeat(max - 4) + emoji
        assertEquals(listOf(Text(fits)), Text.split(fits))

        for (pad in 1..3) {
            val text = "a".repeat(max - 4 + pad) + emoji + "z"
            val chunks = Text.split(text)
            assertEquals("pad $pad", listOf("a".repeat(max - 4 + pad), emoji + "z"), chunks.map { it.text })
            assertValidSplit(chunks, text)
        }
    }

    @Test
    fun emojiSequenceMaySpanMessagesButEachCodePointStaysWhole() {
        val man = "👨"
        val woman = "👩"
        val girl = "👧"
        val zwj = "\u200D"
        val family = man + zwj + woman + zwj + girl // 3 × 4 + 2 × 3 = 18 bytes
        val text = "a".repeat(max - 6) + family // 6 bytes left: the first emoji, not its ZWJ
        val chunks = Text.split(text)
        assertValidSplit(chunks, text)
        assertEquals(listOf("a".repeat(max - 6) + man, zwj + woman + zwj + girl), chunks.map { it.text })
    }

    @Test
    fun unpairedSurrogatesBecomeReplacementCharacters() {
        val chunks = Text.split("a\uD83Db\uDE00")
        assertEquals(listOf(Text("a\uFFFDb\uFFFD")), chunks)
        assertValidSplit(chunks, "a\uFFFDb\uFFFD")
    }

    @Test
    fun customLimit() {
        assertEquals(listOf("abcd", "e"), Text.split("abcde", maxBytes = 4).map { it.text })
        assertEquals(listOf("é", "😀", "é"), Text.split("é😀é", maxBytes = 4).map { it.text })
    }
}
