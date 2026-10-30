package com.achreftlili.sidecast.ui

import android.view.View

/**
 * Keeps [view] at [activeAlpha] while the user interacts, and fades it to [idleAlpha] after
 * [idleMs] without a [poke], so a floating control stays findable without covering the stream.
 */
internal class IdleFader(
    private val view: View,
    private val activeAlpha: Float = 0.5f,
    /** May change at any time (e.g. a mode indicator that should not fade); applies on the next fade. */
    var idleAlpha: Float = 0.25f,
    private val idleMs: Long = 3_000,
) {
    private val fade = Runnable { view.animate().alpha(idleAlpha).setDuration(FADE_OUT_MS) }

    /** Any interaction: back to [activeAlpha], restart the idle timer. */
    fun poke() {
        view.removeCallbacks(fade)
        view.animate().alpha(activeAlpha).setDuration(FADE_IN_MS)
        view.postDelayed(fade, idleMs)
    }

    fun stop() {
        view.removeCallbacks(fade)
        view.animate().cancel()
    }

    private companion object {
        const val FADE_IN_MS = 120L
        const val FADE_OUT_MS = 400L
    }
}
