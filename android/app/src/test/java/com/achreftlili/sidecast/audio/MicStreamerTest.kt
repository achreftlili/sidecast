package com.achreftlili.sidecast.audio

import com.achreftlili.sidecast.net.waitUntil
import com.achreftlili.sidecast.protocol.Audio
import com.achreftlili.sidecast.protocol.MicControl
import com.achreftlili.sidecast.protocol.MicReason
import com.achreftlili.sidecast.protocol.MicState
import com.achreftlili.sidecast.protocol.Protocol
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/** Produces 480-sample chunks every ~2 ms; chunk n is filled with n. */
private class FakeRecorder(private val startOk: Boolean = true, private val failAfterReads: Int = -1) : MicRecorder {
    @Volatile
    var released = false
    private var reads = 0

    override fun start(): Boolean = startOk

    override fun read(buffer: ShortArray): Int {
        Thread.sleep(2)
        if (failAfterReads in 0..reads) return -3 // AudioRecord.ERROR_INVALID_OPERATION
        buffer.fill(reads.toShort())
        reads++
        return buffer.size
    }

    override fun release() {
        released = true
    }
}

private class FakeRecorders : MicRecorderFactory {
    val created = CopyOnWriteArrayList<FakeRecorder>()

    @Volatile
    var make: () -> FakeRecorder? = { FakeRecorder() }

    override fun create(): MicRecorder? = make()?.also { created += it }
}

private class FakePermission(@Volatile var granted: Boolean) : MicPermission {
    val requests = CopyOnWriteArrayList<(Boolean) -> Unit>()

    override fun isGranted(): Boolean = granted

    override fun request(onResult: (Boolean) -> Unit): Boolean {
        requests += onResult
        return true
    }
}

class MicStreamerTest {
    private val events = LinkedBlockingQueue<Any>()
    private val discards = AtomicInteger()
    private val sink = object : MicSink {
        override fun sendMicState(state: MicState) {
            events += state
        }

        override fun sendAudio(audio: Audio) {
            events += audio
        }

        override fun discardPendingAudio() {
            discards.incrementAndGet()
        }
    }

    @Volatile
    private var enabled = true
    private val permission = FakePermission(granted = true)
    private val recorders = FakeRecorders()
    private val streamer = MicStreamer(sink, recorders, permission, isEnabled = { enabled })

    private val start = MicControl(active = true, sampleRate = Protocol.MIC_SAMPLE_RATE, channels = 1)
    private val stop = MicControl(active = false)

    @After
    fun tearDown() = streamer.release()

    private fun poll(timeoutMs: Long = 2_000): Any =
        events.poll(timeoutMs, TimeUnit.MILLISECONDS) ?: throw AssertionError("timed out waiting for an event")

    private fun nextAudio(): Audio = poll().let { it as? Audio ?: throw AssertionError("expected AUDIO, got $it") }

    private fun nextState(): MicState = poll().let { it as? MicState ?: throw AssertionError("expected MIC_STATE, got $it") }

    /** Skips AUDIO already queued, returns the next MIC_STATE. */
    private fun nextStateSkippingAudio(): MicState {
        while (true) {
            val e = poll()
            if (e is MicState) return e
        }
    }

    private fun assertQuiet(ms: Long = 150) {
        assertNull("unexpected event", events.poll(ms, TimeUnit.MILLISECONDS))
    }

    private fun awaitCapturing() {
        assertEquals(0L, nextAudio().seq)
        assertEquals("capturing:true follows the first AUDIO", MicState(true), nextState())
        assertTrue(streamer.capturing.value)
    }

    @Test
    fun capturesWhileTheMacAsksThenStops() {
        streamer.onForeground()
        streamer.onMicControl(start)
        val first = events.poll(2, TimeUnit.SECONDS) as Audio
        assertEquals(0L, first.seq)
        assertEquals(Protocol.MIC_CHUNK_SAMPLES, first.samples.size)
        assertEquals(MicState(true), nextState())
        assertEquals(1L, nextAudio().seq)
        assertEquals(2L, nextAudio().seq)

        streamer.onMicControl(stop)
        assertEquals(MicState(false, MicReason.STOPPED), nextStateSkippingAudio())
        assertQuiet() // never AUDIO after the stop report
        waitUntil(what = "recorder released") { recorders.created.single().released }
        assertFalse(streamer.capturing.value)
        assertTrue("queued audio discarded", discards.get() >= 1)
    }

    @Test
    fun disabledSettingRefusesWithoutTouchingTheMic() {
        enabled = false
        streamer.onForeground()
        streamer.onMicControl(start)
        assertEquals(MicState(false, MicReason.DISABLED), nextState())
        assertQuiet()
        assertTrue(recorders.created.isEmpty())
        assertTrue(permission.requests.isEmpty())
    }

    @Test
    fun permissionIsRequestedThenCaptureStarts() {
        permission.granted = false
        streamer.onForeground()
        streamer.onMicControl(start)
        assertQuiet() // waiting for the user
        assertEquals(1, permission.requests.size)
        streamer.onMicControl(start) // a repeated request while the dialog is up does not ask twice
        assertEquals(1, permission.requests.size)

        permission.granted = true
        permission.requests[0](true)
        awaitCapturing()
    }

    @Test
    fun deniedPermissionIsReportedAndAskedAgainOnTheNextRequest() {
        permission.granted = false
        streamer.onForeground()
        streamer.onMicControl(start)
        permission.requests[0](false)
        assertEquals(MicState(false, MicReason.PERMISSION_DENIED), nextState())
        assertTrue(recorders.created.isEmpty())

        streamer.onMicControl(stop)
        assertEquals(MicState(false, MicReason.STOPPED), nextState())
        streamer.onMicControl(start)
        assertEquals(2, permission.requests.size)
    }

    @Test
    fun stopWhileTheDialogIsUpCancelsTheCapture() {
        permission.granted = false
        streamer.onForeground()
        streamer.onMicControl(start)
        streamer.onMicControl(stop)
        assertEquals(MicState(false, MicReason.STOPPED), nextState())
        permission.granted = true
        permission.requests[0](true)
        assertQuiet()
        assertTrue(recorders.created.isEmpty())
    }

    @Test
    fun backgroundStopsAndForegroundResumesWithoutResettingSeq() {
        streamer.onForeground()
        streamer.onMicControl(start)
        awaitCapturing()
        streamer.onBackground()
        assertEquals(MicState(false, MicReason.BACKGROUND), nextStateSkippingAudio())
        assertQuiet()
        waitUntil(what = "first recorder released") { recorders.created[0].released }
        assertFalse(streamer.capturing.value)

        streamer.onForeground()
        val resumed = nextAudio()
        assertTrue("seq continues (gap, not restart): ${resumed.seq}", resumed.seq > 0)
        assertEquals(MicState(true), nextState())
        assertEquals(2, recorders.created.size)
    }

    @Test
    fun requestWhileInTheBackgroundWaitsForTheForeground() {
        streamer.onMicControl(start) // activity not started yet
        assertEquals(MicState(false, MicReason.BACKGROUND), nextState())
        assertTrue(recorders.created.isEmpty())
        streamer.onForeground()
        awaitCapturing()
    }

    @Test
    fun unavailableMicrophoneIsReported() {
        streamer.onForeground()
        recorders.make = { null } // AudioRecord could not be created (busy / unsupported)
        streamer.onMicControl(start)
        assertEquals(MicState(false, MicReason.UNAVAILABLE), nextState())

        recorders.make = { FakeRecorder(startOk = false) }
        streamer.onMicControl(start)
        assertEquals(MicState(false, MicReason.UNAVAILABLE), nextState())
        waitUntil(what = "failed recorder released") { recorders.created.single().released }

        recorders.make = { FakeRecorder(failAfterReads = 2) } // fails mid-stream
        streamer.onMicControl(start)
        awaitCapturing()
        assertEquals(1L, nextAudio().seq)
        assertEquals(MicState(false, MicReason.UNAVAILABLE), nextState())
        assertFalse(streamer.capturing.value)
        assertQuiet()
    }

    @Test
    fun sessionEndStopsSilently() {
        streamer.onForeground()
        streamer.onMicControl(start)
        awaitCapturing()
        streamer.onMicSessionEnded()
        waitUntil(what = "recorder released") { recorders.created.single().released }
        while (true) {
            val e = events.poll() ?: break
            if (e is MicState) fail("no MIC_STATE on a closed connection, got $e")
        }
        assertQuiet()
        assertFalse(streamer.capturing.value)
    }

    @Test
    fun newRequestRestartsSeqAndReannouncesWithoutReopeningTheMic() {
        streamer.onForeground()
        streamer.onMicControl(start)
        awaitCapturing()
        assertEquals(1L, nextAudio().seq)
        streamer.onMicControl(start)
        while (true) {
            val e = poll()
            if (e is Audio && e.seq == 0L) break
        }
        assertEquals(MicState(true), nextState())
        assertEquals(1, recorders.created.size)
    }

    @Test
    fun backgroundWithoutARequestSaysNothing() {
        streamer.onForeground()
        streamer.onBackground()
        streamer.onForeground()
        assertQuiet()
    }
}
