package com.achreftlili.sidecast.input

import kotlin.math.roundToInt

/** Where the video is drawn inside its container, in container pixels. */
data class VideoRect(val left: Int, val top: Int, val width: Int, val height: Int) {
    /** Container point -> video-normalized point (0..1 inside the picture, beyond in the bars). */
    fun normalizeX(x: Float): Float = if (width > 0) (x - left) / width else 0f

    fun normalizeY(y: Float): Float = if (height > 0) (y - top) / height else 0f

    /** Video-normalized point -> container point (inverse of [normalizeX] / [normalizeY]). */
    fun toContainerX(nx: Float): Float = left + nx * width

    fun toContainerY(ny: Float): Float = top + ny * height
}

/** Aspect-preserving fit ("letterbox"/"pillarbox"), shared by layout and touch mapping. */
object Letterbox {
    fun fit(containerWidth: Int, containerHeight: Int, videoWidth: Int, videoHeight: Int): VideoRect {
        if (containerWidth <= 0 || containerHeight <= 0) return VideoRect(0, 0, 0, 0)
        if (videoWidth <= 0 || videoHeight <= 0) return VideoRect(0, 0, containerWidth, containerHeight)
        val scale = minOf(containerWidth.toDouble() / videoWidth, containerHeight.toDouble() / videoHeight)
        val w = (videoWidth * scale).roundToInt().coerceIn(1, containerWidth)
        val h = (videoHeight * scale).roundToInt().coerceIn(1, containerHeight)
        return VideoRect((containerWidth - w) / 2, (containerHeight - h) / 2, w, h)
    }
}
