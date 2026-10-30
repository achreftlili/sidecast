package com.achreftlili.sidecast.audio

import com.achreftlili.sidecast.net.MicHandler
import com.achreftlili.sidecast.protocol.Audio
import com.achreftlili.sidecast.protocol.MicControl
import com.achreftlili.sidecast.protocol.MicReason
import com.achreftlili.sidecast.protocol.MicState
import com.achreftlili.sidecast.protocol.Protocol
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** One microphone capture, 48 kHz mono PCM16. AudioRecord on the phone, faked in tests. */
interface MicRecorder {
    /** Starts recording; false when the microphone is unavailable. */
    fun start(): Boolean

    /** Blocks until [buffer] is full (10 ms); returns the samples read, or < 0 on error. */
    fun read(buffer: ShortArray): Int

    /** Stops and frees everything. Called once, on the capture thread. */
    fun release()
}

/** Creates a [MicRecorder], or returns null when none can be created (busy / unsupported). */
fun interface MicRecorderFactory {
    fun create(): MicRecorder?
}

/** Where the microphone's traffic goes: the stream that asked for it. Thread-safe, non-blocking. */
interface MicSink {
    fun sendMicState(state: MicState)

    fun sendAudio(audio: Audio)

    /** Drop audio queued but not yet sent (capture stopped). */
    fun discardPendingAudio()
}

/** The RECORD_AUDIO runtime permission. */
interface MicPermission {
    fun isGranted(): Boolean

    /**
     * Asks the user; [onResult] arrives later on any thread. Returns false when it cannot ask now
     * (nothing on screen to ask from).
     */
    fun request(onResult: (Boolean) -> Unit): Boolean
}

/**
 * Phone microphone -> "Sidecast Microphone" on the Mac (§7.2). Pure JVM state machine: captures
 * only while the Mac asks (MIC_CONTROL active:true), the user setting allows it, RECORD_AUDIO is
 * granted and the display screen is in the foreground; reports every refusal with MIC_STATE.
 *
 * Capture runs on its own thread (`sidecast-mic`): 480-sample reads, one AUDIO each, seq from 0
 * after each active:true; MIC_STATE capturing:true follows the first AUDIO. The capture thread only
 * enqueues (the sink is non-blocking). A new capture waits for the previous recorder's release.
 * All other methods may be called from any thread.
 */
class MicStreamer(
    private val sink: MicSink,
    private val recorders: MicRecorderFactory,
    private val permission: MicPermission,
    private val isEnabled: () -> Boolean,
    private val threadSetup: () -> Unit = {},
    private val log: (String) -> Unit = {},
) : MicHandler {
    private class CaptureJob {
        @Volatile
        var active = true
    }

    private val lock = Any()

    // Guarded by [lock].
    private var wanted = false
    private var foreground = false
    private var permissionPending = false
    private var permissionDenied = false
    private var failed = false
    private var released = false
    private var job: CaptureJob? = null
    private var captureThread: Thread? = null
    private var seq = 0L
    private var announced = false
    private var lastReported: MicState? = null

    private val _capturing = MutableStateFlow(false)

    /** True while audio is flowing to the Mac (drives the on-screen indicator). */
    val capturing: StateFlow<Boolean> = _capturing.asStateFlow()

    override fun onMicControl(control: MicControl) {
        synchronized(lock) {
            if (released) return
            if (control.active) {
                // A fresh request: seq restarts at 0 and every earlier refusal is re-evaluated. A
                // capture already running keeps going and re-announces itself with its next chunk.
                wanted = true
                seq = 0
                announced = false
                permissionDenied = false
                failed = false
                lastReported = null
                evaluate()
            } else {
                val was = wanted
                wanted = false
                permissionPending = false
                stopCapture()
                if (was) report(MicState(false, MicReason.STOPPED))
            }
        }
    }

    override fun onMicSessionEnded() {
        synchronized(lock) {
            wanted = false
            permissionPending = false
            stopCapture()
            lastReported = null
        }
    }

    /** The display screen is visible (onStart): resume if the Mac still wants the mic. */
    fun onForeground() {
        synchronized(lock) {
            foreground = true
            failed = false // the microphone may be free again
            if (wanted) lastReported = null
            evaluate()
        }
    }

    /** The display screen is no longer visible (onStop): never record in the background. */
    fun onBackground() {
        synchronized(lock) {
            foreground = false
            if (!wanted) return
            stopCapture()
            report(MicState(false, MicReason.BACKGROUND))
        }
    }

    /** Terminal (the screen's ViewModel is cleared). */
    fun release() {
        synchronized(lock) {
            released = true
            wanted = false
            stopCapture()
        }
    }

    private fun onPermissionResult(granted: Boolean) {
        synchronized(lock) {
            permissionPending = false
            if (!granted) permissionDenied = true
            evaluate()
        }
    }

    /** Brings the capture in line with the current conditions. Holds [lock]. */
    private fun evaluate() {
        if (!wanted || released) return
        when {
            !isEnabled() -> {
                stopCapture()
                report(MicState(false, MicReason.DISABLED))
            }
            !foreground -> report(MicState(false, MicReason.BACKGROUND))
            job != null -> Unit // capturing (or starting)
            failed -> report(MicState(false, MicReason.UNAVAILABLE))
            permission.isGranted() -> startCapture()
            permissionDenied -> report(MicState(false, MicReason.PERMISSION_DENIED))
            permissionPending -> Unit // the system dialog is up
            else -> {
                permissionPending = true
                if (!permission.request(::onPermissionResult)) {
                    permissionPending = false
                    permissionDenied = true
                    report(MicState(false, MicReason.PERMISSION_DENIED))
                }
            }
        }
    }

    private fun startCapture() {
        val newJob = CaptureJob()
        val previous = captureThread
        job = newJob
        captureThread = Thread({ runCapture(newJob, previous) }, "sidecast-mic").apply {
            isDaemon = true
            start()
        }
        log("mic capture starting")
    }

    private fun stopCapture() {
        val j = job ?: return
        job = null
        j.active = false
        announced = false
        _capturing.value = false
        sink.discardPendingAudio()
        log("mic capture stopped")
    }

    private fun report(state: MicState) {
        if (state == lastReported) return
        lastReported = state
        sink.sendMicState(state)
    }

    // ------------------------------------------------------------------ capture thread

    private fun runCapture(job: CaptureJob, previous: Thread?) {
        // Only one recorder at a time: the previous one releases within a read (10 ms).
        previous?.join(PREVIOUS_RELEASE_TIMEOUT_MS)
        threadSetup()
        if (!job.active) return
        val recorder = try {
            recorders.create()
        } catch (e: RuntimeException) {
            log("mic recorder creation failed: $e")
            null
        }
        if (recorder == null) {
            fail(job)
            return
        }
        try {
            if (!recorder.start()) {
                fail(job)
                return
            }
            val buffer = ShortArray(Protocol.MIC_CHUNK_SAMPLES)
            var emptyReads = 0
            while (job.active) {
                val n = recorder.read(buffer)
                when {
                    n < 0 -> {
                        log("mic read error $n")
                        fail(job)
                        return
                    }
                    n == 0 -> if (++emptyReads > MAX_EMPTY_READS) {
                        fail(job)
                        return
                    }
                    else -> {
                        emptyReads = 0
                        deliver(job, buffer, n)
                    }
                }
            }
        } catch (e: RuntimeException) {
            log("mic capture failed: $e")
            fail(job)
        } finally {
            recorder.release()
        }
    }

    private fun deliver(job: CaptureJob, buffer: ShortArray, count: Int) {
        synchronized(lock) {
            if (job !== this.job) return // stopped meanwhile: nothing may follow the stop report
            sink.sendAudio(Audio(seq, buffer.copyOf(count)))
            seq = (seq + 1) and 0xFFFF_FFFFL
            if (!announced) {
                announced = true
                _capturing.value = true
                report(MicState(true))
            }
        }
    }

    private fun fail(job: CaptureJob) {
        synchronized(lock) {
            if (job !== this.job) return
            this.job = null
            announced = false
            _capturing.value = false
            failed = true
            report(MicState(false, MicReason.UNAVAILABLE))
        }
    }

    private companion object {
        const val PREVIOUS_RELEASE_TIMEOUT_MS = 1_000L
        const val MAX_EMPTY_READS = 100
    }
}
