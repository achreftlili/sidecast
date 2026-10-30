package com.achreftlili.sidecast.video

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.Process
import android.util.Log
import android.view.Surface
import com.achreftlili.sidecast.protocol.CodecName
import com.achreftlili.sidecast.protocol.StreamConfig
import com.achreftlili.sidecast.protocol.VideoFrame
import java.util.concurrent.LinkedBlockingDeque
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/** Counters shared by successive decoder instances (stats overlay). */
class DecoderStats {
    val rendered = AtomicLong()
    val droppedLate = AtomicLong()
    val droppedInput = AtomicLong()
}

/**
 * Asynchronous MediaCodec decoder rendering straight into a Surface. Ported from SideScreen's
 * VideoDecoder: hardware decoder preference, low-latency format keys with configure fallbacks,
 * immediate `releaseOutputBuffer(i, true)` and dropping of frames that come out hopelessly late.
 *
 * Parameter sets: on the first keyframe after configure the VPS/SPS/PPS found in the access unit
 * are submitted once as a BUFFER_FLAG_CODEC_CONFIG buffer, then the whole AU (parameter sets
 * included) is queued as a normal buffer. Decoders that want out-of-band csd get it; decoders
 * that parse in-band headers (what SideScreen relies on) see them too.
 *
 * Threading: [start], [submit] and [release] are called by [VideoPipeline] under its lock;
 * codec callbacks run on a private HandlerThread.
 */
class VideoDecoder(
    private val surface: Surface,
    private val config: StreamConfig,
    private val operatingRate: Int,
    private val stats: DecoderStats,
    private val events: Events,
) {
    interface Events {
        /** Ask the Mac for a keyframe (the pipeline throttles). */
        fun onKeyframeNeeded(reason: String)

        /** First frame rendered by this decoder instance. */
        fun onFirstFrameRendered()

        /** The codec is unusable; the pipeline recreates it on the next frame. */
        fun onCodecError(error: Exception)
    }

    private val mime = requireNotNull(DecoderCapabilities.mimeFor(config.codec)) { "unsupported codec ${config.codec}" }
    private val hevc = config.codec == CodecName.HEVC
    private var codec: MediaCodec? = null
    private var thread: HandlerThread? = null
    private val freeInputs = LinkedBlockingDeque<Int>()

    @Volatile
    var isFailed = false
        private set
    private var needsKeyframe = true
    private var parameterSetsSubmitted = false
    private var outputCount = 0L // decoder thread only

    /** Creates, configures (with fallbacks) and starts the codec. Throws when nothing works. */
    fun start() {
        val name = DecoderCapabilities.selectDecoder(mime, config.width, config.height, config.fps)
        val created = if (name != null) MediaCodec.createByCodecName(name) else MediaCodec.createDecoderByType(mime)
        val handlerThread = HandlerThread("sidecast-decoder", Process.THREAD_PRIORITY_DISPLAY).also { it.start() }
        thread = handlerThread
        val handler = Handler(handlerThread.looper)
        val callback = Callback()
        try {
            var lastError: Exception? = null
            var configured = false
            for ((label, format) in formats(created.name)) {
                try {
                    created.setCallback(callback, handler)
                    created.configure(format, surface, null, 0)
                    Log.i(TAG, "configured ${created.name} (${config.width}x${config.height} ${config.codec}) with $label format")
                    configured = true
                    break
                } catch (e: Exception) {
                    Log.w(TAG, "$label configure of ${created.name} failed: ${e.message}")
                    lastError = e
                    created.reset()
                }
            }
            if (!configured) throw lastError ?: IllegalStateException("configure failed")
            created.setVideoScalingMode(MediaCodec.VIDEO_SCALING_MODE_SCALE_TO_FIT)
            created.start()
            codec = created
        } catch (e: Exception) {
            created.release()
            handlerThread.quitSafely()
            thread = null
            throw e
        }
    }

    private fun formats(codecName: String): List<Pair<String, MediaFormat>> {
        fun base() = MediaFormat.createVideoFormat(mime, config.width, config.height)
        // Room for a keyframe at high bitrate (1 byte per pixel is far above any sane AU) without
        // making the codec allocate huge buffers for 4K+ streams.
        val maxInput = (config.width * config.height).coerceIn(1 shl 20, 6 shl 20)
        val lowLatency = base().apply {
            setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, maxInput)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0) // realtime
            setInteger(MediaFormat.KEY_OPERATING_RATE, operatingRate)
            val lower = codecName.lowercase()
            if (lower.startsWith("c2.qti.") || lower.startsWith("omx.qcom.")) {
                // Qualcomm vendor low-latency mode (pre-Android 11 devices ignore KEY_LOW_LATENCY).
                setInteger("vendor.qti-ext-dec-picture-order.enable", 1)
                setInteger("vendor.qti-ext-dec-low-latency.enable", 1)
            }
        }
        val basic = base().apply {
            setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, maxInput)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
        }
        return listOf("low-latency" to lowLatency, "basic" to basic, "minimal" to base())
    }

    /** Called on the network reader thread for every received frame (already acked). */
    fun submit(frame: VideoFrame, receivedAtNanos: Long) {
        val codec = codec ?: return
        if (isFailed) return
        if (needsKeyframe && !frame.isKeyframe) {
            stats.droppedInput.incrementAndGet()
            events.onKeyframeNeeded("waiting for keyframe")
            return
        }
        val ptsUs = receivedAtNanos / 1_000
        if (frame.isKeyframe && !parameterSetsSubmitted) {
            val csd = AnnexB.parameterSets(frame.data, frame.dataOffset, frame.dataLength, hevc)
            if (csd != null) {
                if (!queue(codec, csd, 0, csd.size, 0, MediaCodec.BUFFER_FLAG_CODEC_CONFIG, KEYFRAME_WAIT_MS)) return
            }
            parameterSetsSubmitted = true
        }
        val wait = if (frame.isKeyframe) KEYFRAME_WAIT_MS else DELTA_WAIT_MS
        if (queue(codec, frame.data, frame.dataOffset, frame.dataLength, ptsUs, 0, wait) && frame.isKeyframe) {
            needsKeyframe = false
        }
    }

    /** False when the frame was dropped; a keyframe is then requested and awaited. */
    private fun queue(codec: MediaCodec, data: ByteArray, offset: Int, length: Int, ptsUs: Long, flags: Int, waitMs: Long): Boolean {
        val index = freeInputs.poll(waitMs, TimeUnit.MILLISECONDS)
        if (index == null) {
            dropUntilKeyframe("decoder input full")
            return false
        }
        try {
            val buffer = codec.getInputBuffer(index) ?: throw IllegalStateException("input buffer $index is null")
            if (length > buffer.capacity()) {
                freeInputs.offerFirst(index) // still ours: never queued
                Log.w(TAG, "access unit of $length bytes exceeds input buffer ${buffer.capacity()}")
                dropUntilKeyframe("frame too large")
                return false
            }
            buffer.clear()
            buffer.put(data, offset, length)
            codec.queueInputBuffer(index, 0, length, ptsUs, flags)
            return true
        } catch (e: IllegalStateException) {
            // Includes MediaCodec.CodecException; onError usually follows.
            Log.w(TAG, "queueInputBuffer failed: ${e.message}")
            dropUntilKeyframe("queue failed")
            return false
        }
    }

    private fun dropUntilKeyframe(reason: String) {
        stats.droppedInput.incrementAndGet()
        needsKeyframe = true
        events.onKeyframeNeeded(reason)
    }

    fun release() {
        val c = codec
        codec = null
        if (c != null) {
            try {
                c.stop()
            } catch (_: Exception) {
            }
            try {
                c.release()
            } catch (_: Exception) {
            }
        }
        thread?.quitSafely()
        thread = null
        freeInputs.clear()
    }

    private inner class Callback : MediaCodec.Callback() {
        override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {
            freeInputs.offer(index)
        }

        override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            try {
                val count = ++outputCount
                // pts carries the receive time, so this is network-receipt -> decoded latency.
                val ageNs = System.nanoTime() - info.presentationTimeUs * 1_000
                val render = count == 1L || ageNs !in 0..MAX_PLAUSIBLE_AGE_NS || ageNs <= MAX_RENDER_AGE_NS
                codec.releaseOutputBuffer(index, render)
                if (render) {
                    stats.rendered.incrementAndGet()
                    if (count == 1L) events.onFirstFrameRendered()
                } else {
                    stats.droppedLate.incrementAndGet()
                }
            } catch (_: IllegalStateException) {
                // Codec stopped/released concurrently.
            }
        }

        override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
            Log.e(TAG, "codec error: ${e.diagnosticInfo} transient=${e.isTransient} recoverable=${e.isRecoverable}", e)
            isFailed = true
            events.onCodecError(e)
        }

        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
            Log.i(TAG, "output format: $format")
        }
    }

    private companion object {
        const val TAG = "Sidecast/Decoder"

        /** Right after configure the codec may not have offered its input buffers yet. */
        const val KEYFRAME_WAIT_MS = 100L
        const val DELTA_WAIT_MS = 20L

        /** Frames older than this when decoded are skipped instead of shown (SideScreen: 100 ms). */
        const val MAX_RENDER_AGE_NS = 100_000_000L
        const val MAX_PLAUSIBLE_AGE_NS = 2_000_000_000L
    }
}
