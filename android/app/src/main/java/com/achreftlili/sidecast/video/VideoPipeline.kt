package com.achreftlili.sidecast.video

import android.util.Log
import android.view.Surface
import com.achreftlili.sidecast.net.VideoSink
import com.achreftlili.sidecast.protocol.StreamConfig
import com.achreftlili.sidecast.protocol.VideoFrame
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** What the decoder side is doing, for the overlay. */
sealed interface DecoderStatus {
    data object Idle : DecoderStatus

    data class Running(val config: StreamConfig, val hasPicture: Boolean) : DecoderStatus

    data class Unsupported(val config: StreamConfig, val detail: String) : DecoderStatus
}

/**
 * Glues the network stream to the screen: a decoder exists exactly when both a Surface and a
 * STREAM_CONFIG are present. Reconfigures on every STREAM_CONFIG (§6), recreates the codec after
 * errors and asks the Mac for a keyframe after each (re)configure.
 */
class VideoPipeline(
    private val operatingRate: Int,
    private val keyframeRequester: () -> Unit,
) : VideoSink {
    private val lock = Any()
    private var surface: Surface? = null
    private var config: StreamConfig? = null
    private var decoder: VideoDecoder? = null

    @Volatile
    private var lastKeyframeRequestNanos = 0L

    val stats = DecoderStats()

    private val _status = MutableStateFlow<DecoderStatus>(DecoderStatus.Idle)
    val status: StateFlow<DecoderStatus> = _status.asStateFlow()

    private val events = object : VideoDecoder.Events {
        override fun onKeyframeNeeded(reason: String) = requestKeyframe(reason, force = false)

        override fun onFirstFrameRendered() {
            val current = _status.value
            if (current is DecoderStatus.Running && !current.hasPicture) _status.value = current.copy(hasPicture = true)
        }

        override fun onCodecError(error: Exception) {
            requestKeyframe("codec error", force = false)
        }
    }

    /** Main thread, from SurfaceHolder callbacks. Null when the surface is destroyed. */
    fun setSurface(newSurface: Surface?) {
        synchronized(lock) {
            if (newSurface === surface) return
            releaseDecoder()
            surface = newSurface
            ensureDecoder("surface changed")
        }
    }

    override fun onStreamConfig(config: StreamConfig) {
        synchronized(lock) {
            this.config = config
            releaseDecoder()
            ensureDecoder("stream config")
        }
    }

    override fun onVideoFrame(frame: VideoFrame, receivedAtNanos: Long) {
        synchronized(lock) {
            val current = decoder
            if (current != null && current.isFailed) {
                releaseDecoder()
                ensureDecoder("decoder error")
            }
            val active = decoder
            if (active != null) active.submit(frame, receivedAtNanos) else stats.droppedInput.incrementAndGet()
        }
    }

    fun release() {
        synchronized(lock) {
            releaseDecoder()
            surface = null
            _status.value = DecoderStatus.Idle
        }
    }

    private fun ensureDecoder(reason: String) {
        val s = surface?.takeIf { it.isValid } ?: return
        val c = config ?: return
        val d = VideoDecoder(s, c, operatingRate.coerceAtLeast(c.fps), stats, events)
        try {
            d.start()
            decoder = d
            _status.value = DecoderStatus.Running(c, hasPicture = false)
            Log.i(TAG, "decoder ready ($reason): $c")
            requestKeyframe(reason, force = true)
        } catch (e: Exception) {
            Log.e(TAG, "cannot decode $c", e)
            d.release()
            _status.value = DecoderStatus.Unsupported(c, e.message ?: e.javaClass.simpleName)
        }
    }

    private fun releaseDecoder() {
        decoder?.release()
        decoder = null
    }

    private fun requestKeyframe(reason: String, force: Boolean) {
        val now = System.nanoTime()
        if (!force && now - lastKeyframeRequestNanos < KEYFRAME_THROTTLE_NS) return
        lastKeyframeRequestNanos = now
        Log.d(TAG, "KEYFRAME_REQUEST: $reason")
        keyframeRequester()
    }

    private companion object {
        const val TAG = "Sidecast/Video"
        const val KEYFRAME_THROTTLE_NS = 500_000_000L
    }
}
