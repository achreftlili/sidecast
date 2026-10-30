package com.achreftlili.sidecast.video

import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat
import android.util.Log
import com.achreftlili.sidecast.protocol.CodecName
import com.achreftlili.sidecast.protocol.CodecSupport
import com.achreftlili.sidecast.protocol.ScreenInfo

/**
 * Decoder probing (ported from SideScreen's CodecCapabilities / VideoDecoder.findBestDecoder).
 *
 * "Usable hardware" excludes the Google software codecs (far too slow for real-time) and the
 * Spreadtrum/Unisoc HEVC decoders that start fine but never render to a Surface.
 */
object DecoderCapabilities {
    private const val TAG = "Sidecast/Codecs"
    private const val REFERENCE_FPS = 60.0
    private const val MAX_PROBE_WIDTH = 7680
    private const val MIN_USEFUL_WIDTH = 640

    /** H.264 must sustain this much more area than HEVC before it is preferred. */
    private const val AVC_PREFERENCE_AREA_GAIN = 1.2

    private val BROKEN_HEVC_PREFIXES = listOf("omx.sprd.", "c2.sprd.")
    private val SOFTWARE_PREFIXES = listOf("c2.android.", "omx.google.")

    fun mimeFor(codec: String): String? = when (codec) {
        CodecName.HEVC -> MediaFormat.MIMETYPE_VIDEO_HEVC
        CodecName.H264 -> MediaFormat.MIMETYPE_VIDEO_AVC
        else -> null
    }

    private val decoders: List<MediaCodecInfo> by lazy {
        try {
            MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.filter { !it.isEncoder && !it.isAlias }
        } catch (e: RuntimeException) {
            Log.w(TAG, "MediaCodecList failed", e)
            emptyList()
        }
    }

    private fun supports(info: MediaCodecInfo, mime: String) = info.supportedTypes.any { it.equals(mime, ignoreCase = true) }

    private fun isUsableHardware(info: MediaCodecInfo, mime: String): Boolean {
        val name = info.name.lowercase()
        if (!info.isHardwareAccelerated || info.isSoftwareOnly) return false
        if (SOFTWARE_PREFIXES.any { name.startsWith(it) }) return false
        if (mime == MediaFormat.MIMETYPE_VIDEO_HEVC && BROKEN_HEVC_PREFIXES.any { name.startsWith(it) }) return false
        return true
    }

    private fun videoCaps(info: MediaCodecInfo, mime: String): MediaCodecInfo.VideoCapabilities? = try {
        info.getCapabilitiesForType(mime).videoCapabilities
    } catch (_: RuntimeException) {
        null
    }

    /**
     * HELLO `codecs`: per codec, the largest landscape size (screen aspect) some usable hardware
     * decoder sustains at >= 60 fps. HEVC first, unless HEVC cannot cover the screen and H.264
     * sustains a noticeably larger picture (budget SoCs pair a weak HEVC block with a strong AVC
     * one). Software decoders are offered only when the device has no hardware decoder at all.
     */
    fun advertisedCodecs(screen: ScreenInfo): List<CodecSupport> {
        val aspect = if (screen.width > 0 && screen.height > 0) {
            maxOf(screen.width, screen.height).toDouble() / minOf(screen.width, screen.height)
        } else {
            16.0 / 9.0
        }
        fun probe(hardware: Boolean) = listOf(CodecName.HEVC, CodecName.H264).mapNotNull { name ->
            val mime = mimeFor(name)!!
            decoders.asSequence()
                .filter { supports(it, mime) && (!hardware || isUsableHardware(it, mime)) }
                .mapNotNull { info -> videoCaps(info, mime)?.let { maxSizeAt(it, aspect, REFERENCE_FPS) } }
                .maxByOrNull { it.first.toLong() * it.second }
                ?.let { CodecSupport(name, it.first, it.second) }
        }

        var codecs = probe(hardware = true)
        if (codecs.isEmpty()) {
            Log.w(TAG, "no hardware video decoder; advertising software decoders")
            codecs = probe(hardware = false)
        }
        val hevc = codecs.firstOrNull { it.name == CodecName.HEVC }
        val avc = codecs.firstOrNull { it.name == CodecName.H264 }
        val ordered = if (hevc != null && avc != null && preferAvc(hevc, avc, screen)) listOf(avc, hevc) else codecs
        Log.i(TAG, "advertising $ordered for screen ${screen.width}x${screen.height}")
        return ordered
    }

    private fun preferAvc(hevc: CodecSupport, avc: CodecSupport, screen: ScreenInfo): Boolean {
        val hevcArea = hevc.maxWidth.toDouble() * hevc.maxHeight
        val avcArea = avc.maxWidth.toDouble() * avc.maxHeight
        val screenArea = screen.width.toDouble() * screen.height
        return hevcArea < screenArea && avcArea >= hevcArea * AVC_PREFERENCE_AREA_GAIN
    }

    /**
     * Largest size with [aspect] that [caps] decodes at [fps]. Shrinks in 5 % steps because the
     * budget (`blocks-per-second`) counts aligned macroblocks; any size inside the result has
     * fewer blocks, so the result is a safe bounding box for the server.
     */
    private fun maxSizeAt(caps: MediaCodecInfo.VideoCapabilities, aspect: Double, fps: Double): Pair<Int, Int>? {
        val wAlign = maxOf(caps.widthAlignment, 2)
        val hAlign = maxOf(caps.heightAlignment, 2)
        var width = minOf(caps.supportedWidths.upper, MAX_PROBE_WIDTH)
        repeat(80) {
            var w = width
            var h = (w / aspect).toInt()
            if (h > caps.supportedHeights.upper) {
                h = caps.supportedHeights.upper
                w = (h * aspect).toInt()
            }
            val alignedW = w / wAlign * wAlign
            val alignedH = h / hAlign * hAlign
            if (alignedW < MIN_USEFUL_WIDTH) return null
            val ok = try {
                caps.areSizeAndRateSupported(alignedW, alignedH, fps)
            } catch (_: IllegalArgumentException) {
                false
            }
            if (ok) return alignedW to alignedH
            width = (minOf(width, w) * 0.95).toInt()
        }
        return null
    }

    /**
     * Decoder to instantiate for a stream: hardware that sustains the rate, then any hardware
     * that fits the size, then software. Null lets MediaCodec pick its default.
     */
    fun selectDecoder(mime: String, width: Int, height: Int, fps: Int): String? {
        val rate = fps.coerceAtLeast(30).toDouble()
        var hwRate: String? = null
        var hwSize: String? = null
        var swRate: String? = null
        var swSize: String? = null
        for (info in decoders) {
            if (!supports(info, mime)) continue
            val caps = videoCaps(info, mime) ?: continue
            if (!caps.isSizeSupported(width, height)) continue
            val rateOk = try {
                caps.areSizeAndRateSupported(width, height, rate)
            } catch (_: IllegalArgumentException) {
                false
            }
            val hardware = isUsableHardware(info, mime)
            when {
                hardware && rateOk -> hwRate = hwRate ?: info.name
                hardware -> hwSize = hwSize ?: info.name
                rateOk -> swRate = swRate ?: info.name
                else -> swSize = swSize ?: info.name
            }
        }
        return (hwRate ?: hwSize ?: swRate ?: swSize).also {
            Log.i(TAG, "decoder for $mime ${width}x$height@$fps: $it")
        }
    }
}
