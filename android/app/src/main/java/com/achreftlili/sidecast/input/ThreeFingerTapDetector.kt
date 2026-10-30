package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.protocol.Touch
import kotlin.math.hypot

/**
 * Recognizes a quick three-finger tap (used locally to toggle the stats overlay). It only
 * observes: touches are still forwarded to the Mac unchanged.
 */
class ThreeFingerTapDetector(
    private val slopPx: Float,
    private val maxDurationMs: Long = 400,
) {
    private var downTimeMs = 0L
    private var maxPointers = 0
    private var moved = false
    private val origins = HashMap<Int, Pair<Float, Float>>()

    /** Feed every event; returns true on the UP that completes a three-finger tap. */
    fun onEvent(actionMasked: Int, eventTimeMs: Long, pointers: List<PointerSample>): Boolean {
        when (actionMasked) {
            Touch.ACTION_DOWN -> {
                origins.clear()
                downTimeMs = eventTimeMs
                maxPointers = pointers.size
                moved = false
                remember(pointers)
            }
            Touch.ACTION_POINTER_DOWN -> {
                maxPointers = maxOf(maxPointers, pointers.size)
                remember(pointers)
            }
            Touch.ACTION_MOVE -> {
                for (p in pointers) {
                    val (ox, oy) = origins[p.id] ?: continue
                    if (hypot(p.x - ox, p.y - oy) > slopPx) moved = true
                }
            }
            Touch.ACTION_UP -> {
                val tap = maxPointers == 3 && !moved && eventTimeMs - downTimeMs <= maxDurationMs
                origins.clear()
                maxPointers = 0
                return tap
            }
            Touch.ACTION_CANCEL -> {
                origins.clear()
                maxPointers = 0
            }
        }
        return false
    }

    private fun remember(pointers: List<PointerSample>) {
        for (p in pointers) origins.getOrPut(p.id) { p.x to p.y }
    }
}
