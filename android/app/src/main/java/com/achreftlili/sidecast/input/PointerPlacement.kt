package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.protocol.Cursor
import com.achreftlili.sidecast.protocol.InputModeName

/**
 * Where to draw the Mac's pointer on the phone (§7.3), in container pixels, or null to hide it.
 * Pure, so the visibility rules and the letterbox mapping are unit tested.
 */
object PointerPlacement {
    data class Position(val x: Float, val y: Float)

    /**
     * @param cursor the last CURSOR of this connection, null before the first one
     * @param mode the current input mode: the pointer exists only in trackpad mode
     * @param playing the stream is up and visible (no connection / error overlay)
     * @param rect where the video is drawn (same mapping as touches)
     */
    fun position(cursor: Cursor?, mode: String, playing: Boolean, rect: VideoRect): Position? {
        if (cursor == null || !cursor.onDisplay) return null
        if (mode != InputModeName.TRACKPAD || !playing) return null
        if (rect.width <= 0 || rect.height <= 0) return null
        return Position(rect.toContainerX(cursor.x), rect.toContainerY(cursor.y))
    }
}
