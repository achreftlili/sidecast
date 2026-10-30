package com.achreftlili.sidecast.input

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LetterboxTest {
    private val eps = 1e-4f

    @Test
    fun exactFitFillsContainer() {
        assertEquals(VideoRect(0, 0, 2400, 1080), Letterbox.fit(2400, 1080, 2400, 1080))
        // Same aspect, different resolution: scaled up, no bars.
        assertEquals(VideoRect(0, 0, 2400, 1080), Letterbox.fit(2400, 1080, 1200, 540))
    }

    @Test
    fun widerContainerGetsPillarbox() {
        // 16:9 video on a 20:9 phone: bars left and right.
        val rect = Letterbox.fit(2400, 1080, 1920, 1080)
        assertEquals(VideoRect(240, 0, 1920, 1080), rect)
    }

    @Test
    fun tallerContainerGetsLetterbox() {
        // 20:9 video on a 16:10 tablet: bars top and bottom.
        val rect = Letterbox.fit(2560, 1600, 2400, 1080)
        assertEquals(2560, rect.width)
        assertEquals(1152, rect.height)
        assertEquals(0, rect.left)
        assertEquals((1600 - 1152) / 2, rect.top)
    }

    @Test
    fun mapsTouchesToVideoCoordinates() {
        val rect = Letterbox.fit(2400, 1080, 1920, 1080) // left = 240
        assertEquals(0f, rect.normalizeX(240f), eps)
        assertEquals(1f, rect.normalizeX(2160f), eps)
        assertEquals(0.5f, rect.normalizeX(1200f), eps)
        assertEquals(0.25f, rect.normalizeY(270f), eps)
        // Touches in the black bars fall slightly outside 0..1 (the server clamps).
        assertTrue(rect.normalizeX(100f) < 0f)
        assertTrue(rect.normalizeX(2300f) > 1f)
    }

    @Test
    fun letterboxedVerticalMapping() {
        val rect = Letterbox.fit(1000, 1000, 2000, 1000) // 1000x500 at top 250
        assertEquals(VideoRect(0, 250, 1000, 500), rect)
        assertEquals(0f, rect.normalizeY(250f), eps)
        assertEquals(1f, rect.normalizeY(750f), eps)
        assertEquals(0.5f, rect.normalizeY(500f), eps)
    }

    @Test
    fun unknownVideoSizeUsesWholeContainer() {
        assertEquals(VideoRect(0, 0, 800, 600), Letterbox.fit(800, 600, 0, 0))
    }

    @Test
    fun emptyContainerIsSafe() {
        val rect = Letterbox.fit(0, 0, 1920, 1080)
        assertEquals(0f, rect.normalizeX(10f), 0f)
        assertEquals(0f, rect.normalizeY(10f), 0f)
    }
}
