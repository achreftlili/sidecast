package com.achreftlili.sidecast.ui

import android.app.Activity
import android.os.Build
import android.util.DisplayMetrics
import android.view.Display
import com.achreftlili.sidecast.protocol.ScreenInfo

/** HELLO `screen`: the real display in landscape, including system-bar and cutout areas. */
object ScreenMetrics {
    fun landscape(activity: Activity): ScreenInfo {
        val (w, h) = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            activity.windowManager.maximumWindowMetrics.bounds.let { it.width() to it.height() }
        } else {
            val metrics = DisplayMetrics()
            @Suppress("DEPRECATION")
            activity.windowManager.defaultDisplay.getRealMetrics(metrics)
            metrics.widthPixels to metrics.heightPixels
        }
        val display: Display? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            activity.display
        } else {
            @Suppress("DEPRECATION")
            activity.windowManager.defaultDisplay
        }
        return ScreenInfo(
            width = maxOf(w, h),
            height = minOf(w, h),
            densityDpi = activity.resources.displayMetrics.densityDpi,
            refreshRate = display?.let(::peakRefreshRate) ?: 60f,
        )
    }

    /** Fastest mode at the current resolution (adaptive panels idle at a lower rate). */
    private fun peakRefreshRate(display: Display): Float {
        val current = display.mode
        return display.supportedModes
            .filter { it.physicalWidth == current.physicalWidth && it.physicalHeight == current.physicalHeight }
            .maxOfOrNull { it.refreshRate }
            ?: display.refreshRate
    }
}
