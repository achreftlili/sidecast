package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.input.PointerPlacement.Position
import com.achreftlili.sidecast.protocol.Cursor
import com.achreftlili.sidecast.protocol.InputModeName.TOUCH
import com.achreftlili.sidecast.protocol.InputModeName.TRACKPAD
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PointerPlacementTest {
    private val full = Letterbox.fit(2400, 1080, 2400, 1080)

    private fun place(cursor: Cursor?, rect: VideoRect = full, mode: String = TRACKPAD, playing: Boolean = true) =
        PointerPlacement.position(cursor, mode, playing, rect)

    @Test
    fun mapsNormalizedToTheVideoRect() {
        assertEquals(Position(1200f, 270f), place(Cursor(0.5f, 0.25f)))
        assertEquals(Position(0f, 0f), place(Cursor(0f, 0f)))
        assertEquals(Position(2400f, 1080f), place(Cursor(1f, 1f)))
    }

    @Test
    fun pillarboxShiftsXOnly() {
        val rect = Letterbox.fit(2400, 1080, 1920, 1080) // 4:3-ish stream: bars left and right
        assertEquals(VideoRect(240, 0, 1920, 1080), rect)
        assertEquals(Position(240f, 0f), place(Cursor(0f, 0f), rect))
        assertEquals(Position(1200f, 540f), place(Cursor(0.5f, 0.5f), rect))
        assertEquals(Position(2160f, 1080f), place(Cursor(1f, 1f), rect))
    }

    @Test
    fun letterboxShiftsYOnly() {
        val rect = Letterbox.fit(2000, 1200, 2000, 1000) // wider stream: bars top and bottom
        assertEquals(VideoRect(0, 100, 2000, 1000), rect)
        assertEquals(Position(0f, 100f), place(Cursor(0f, 0f), rect))
        assertEquals(Position(500f, 850f), place(Cursor(0.25f, 0.75f), rect))
    }

    @Test
    fun inverseOfTheTouchMapping() {
        val rect = VideoRect(240, 30, 1920, 1020)
        val p = place(Cursor(0.3f, 0.6f), rect)!!
        assertEquals(0.3f, rect.normalizeX(p.x), 1e-6f)
        assertEquals(0.6f, rect.normalizeY(p.y), 1e-6f)
    }

    @Test
    fun hiddenWhenOffThisDisplay() {
        assertNull(place(Cursor(-1f, -1f)))
        assertNull(place(Cursor(1.2f, 0.5f)))
        assertNull(place(Cursor(0.5f, -0.1f)))
        assertNull(place(Cursor(Float.NaN, 0.5f)))
    }

    @Test
    fun hiddenOutsideTrackpadModeOrWhenNotPlayingOrBeforeTheFirstCursor() {
        assertNull(place(Cursor(0.5f, 0.5f), mode = TOUCH))
        assertNull(place(Cursor(0.5f, 0.5f), playing = false)) // reconnect / error overlay, no picture
        assertNull(place(null))
        assertNull(place(Cursor(0.5f, 0.5f), rect = VideoRect(0, 0, 0, 0))) // not laid out yet
    }
}
