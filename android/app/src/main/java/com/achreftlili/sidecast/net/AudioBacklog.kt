package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.Protocol

/**
 * Encoded AUDIO frames waiting for the writer thread. Bounded in audio time: when more than
 * [maxSamples] are pending, the oldest frames are dropped (the newest one is always kept), so a
 * stalled network delays the Mac's microphone by at most ~100 ms and the capture thread never
 * blocks. Thread-safe.
 */
class AudioBacklog(private val maxSamples: Int = DEFAULT_MAX_SAMPLES) {
    private class Pending(val frame: ByteArray, val samples: Int)

    private val queue = ArrayDeque<Pending>()
    private var pending = 0
    private var droppedFrames = 0L

    /** Samples queued and not yet taken by the writer. */
    val pendingSamples: Int @Synchronized get() = pending

    /** Frames dropped so far because the backlog was full. */
    val dropped: Long @Synchronized get() = droppedFrames

    /** Queues [frame] (carrying [samples] samples). Returns how many older frames were dropped. */
    @Synchronized
    fun offer(frame: ByteArray, samples: Int): Int {
        queue.addLast(Pending(frame, samples))
        pending += samples
        var n = 0
        while (pending > maxSamples && queue.size > 1) {
            pending -= queue.removeFirst().samples
            n++
        }
        droppedFrames += n
        return n
    }

    /** Oldest pending frame, or null. */
    @Synchronized
    fun poll(): ByteArray? {
        val p = queue.removeFirstOrNull() ?: return null
        pending -= p.samples
        return p.frame
    }

    @Synchronized
    fun clear() {
        queue.clear()
        pending = 0
    }

    companion object {
        /** 100 ms of 48 kHz audio. */
        const val DEFAULT_MAX_SAMPLES = Protocol.MIC_SAMPLE_RATE / 10
    }
}
