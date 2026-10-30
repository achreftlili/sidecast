package com.achreftlili.sidecast.net

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

class AudioBacklogTest {
    private fun frame(id: Int) = byteArrayOf(id.toByte())

    @Test
    fun keepsAtMost100MsAndDropsTheOldest() {
        val backlog = AudioBacklog() // 4800 samples = 100 ms at 48 kHz
        repeat(10) { assertEquals(0, backlog.offer(frame(it), 480)) }
        assertEquals(4800, backlog.pendingSamples)
        assertEquals(1, backlog.offer(frame(10), 480)) // 110 ms: chunk 0 goes
        assertEquals(1, backlog.offer(frame(11), 480))
        assertEquals(2L, backlog.dropped)
        val left = generateSequence { backlog.poll() }.map { it[0].toInt() }.toList()
        assertEquals((2..11).toList(), left)
        assertEquals(0, backlog.pendingSamples)
        assertNull(backlog.poll())
    }

    @Test
    fun boundIsInSamplesNotFrames() {
        val backlog = AudioBacklog(maxSamples = 1000)
        backlog.offer(frame(0), 600)
        assertEquals(1, backlog.offer(frame(1), 600)) // 1200 > 1000: the 600-sample frame 0 goes
        assertEquals(0, backlog.offer(frame(2), 100))
        assertEquals(700, backlog.pendingSamples)
    }

    @Test
    fun newestFrameIsAlwaysKept() {
        val backlog = AudioBacklog(maxSamples = 100)
        val big = frame(9)
        backlog.offer(frame(0), 50)
        assertEquals(1, backlog.offer(big, 500))
        assertSame(big, backlog.poll())
    }

    @Test
    fun clearForgetsEverything() {
        val backlog = AudioBacklog()
        repeat(3) { backlog.offer(frame(it), 480) }
        backlog.clear()
        assertNull(backlog.poll())
        assertEquals(0, backlog.pendingSamples)
    }
}
