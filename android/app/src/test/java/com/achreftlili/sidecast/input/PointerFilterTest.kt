package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.input.PointerFilter.Event
import com.achreftlili.sidecast.protocol.Touch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PointerFilterTest {
    // A 2400×1080 stream with a "button" at x ≥ 2300, y ≥ 980 and a 40 px edge band.
    private val filter = PointerFilter { x, y, first ->
        (x >= 2300 && y >= 980) || (!first && (x < 40 || x > 2360 || y < 40 || y > 1040))
    }

    private fun p(id: Int, x: Float = 1200f, y: Float = 540f) = PointerSample(id, x, y)

    private val a = p(0)
    private val b = p(1, 1300f, 600f)
    private val onButton = p(1, 2340f, 1020f)
    private val atEdge = p(1, 10f, 500f)

    @Test
    fun singleTapIsUnchanged() {
        assertEquals(Event(Touch.ACTION_DOWN, 0, listOf(a)), filter.filter(Touch.ACTION_DOWN, 0, listOf(a)))
        assertEquals(Event(Touch.ACTION_UP, 0, listOf(a)), filter.filter(Touch.ACTION_UP, 0, listOf(a)))
    }

    @Test
    fun genuineTwoFingerTapIsUnchanged() {
        filter.filter(Touch.ACTION_DOWN, 0, listOf(a))
        assertEquals(Event(Touch.ACTION_POINTER_DOWN, 1, listOf(a, b)), filter.filter(Touch.ACTION_POINTER_DOWN, 1, listOf(a, b)))
        assertEquals(Event(Touch.ACTION_MOVE, 0, listOf(a, b)), filter.filter(Touch.ACTION_MOVE, 0, listOf(a, b)))
        assertEquals(Event(Touch.ACTION_POINTER_UP, 1, listOf(a, b)), filter.filter(Touch.ACTION_POINTER_UP, 1, listOf(a, b)))
        assertEquals(Event(Touch.ACTION_UP, 0, listOf(a)), filter.filter(Touch.ACTION_UP, 0, listOf(a)))
    }

    @Test
    fun secondFingerOnAButtonNeverReachesTheMac() {
        // The bug shape: a finger on the stream, another lands on the keyboard button.
        filter.filter(Touch.ACTION_DOWN, 0, listOf(a))
        assertNull(filter.filter(Touch.ACTION_POINTER_DOWN, 1, listOf(a, onButton)))
        assertEquals(Event(Touch.ACTION_MOVE, 0, listOf(a)), filter.filter(Touch.ACTION_MOVE, 0, listOf(a, onButton)))
        assertNull(filter.filter(Touch.ACTION_POINTER_UP, 1, listOf(a, onButton)))
        assertEquals(Event(Touch.ACTION_UP, 0, listOf(a)), filter.filter(Touch.ACTION_UP, 0, listOf(a)))
    }

    @Test
    fun thumbRestingOnTheEdgeDoesNotTurnATapIntoATwoFingerTap() {
        filter.filter(Touch.ACTION_DOWN, 0, listOf(a))
        assertNull(filter.filter(Touch.ACTION_POINTER_DOWN, 1, listOf(a, atEdge)))
        assertEquals(Event(Touch.ACTION_UP, 0, listOf(a)), filter.filter(Touch.ACTION_POINTER_UP, 0, listOf(a, atEdge)))
        assertNull(filter.filter(Touch.ACTION_UP, 0, listOf(atEdge)))
    }

    @Test
    fun firstFingerMayStartAtTheEdge() {
        assertEquals(Event(Touch.ACTION_DOWN, 0, listOf(atEdge)), filter.filter(Touch.ACTION_DOWN, 0, listOf(atEdge)))
    }

    @Test
    fun aHeldBackFirstPointerHandsTheGestureToTheNextFinger() {
        val chip = p(0, 2350f, 1000f) // down on a non-clickable control (the mic chip)
        val finger = p(1, 1000f, 500f)
        assertNull(filter.filter(Touch.ACTION_DOWN, 0, listOf(chip)))
        assertNull(filter.filter(Touch.ACTION_MOVE, 0, listOf(chip)))
        // Seen by the Mac as the gesture's first finger, at index 0 of what is forwarded.
        assertEquals(Event(Touch.ACTION_DOWN, 0, listOf(finger)), filter.filter(Touch.ACTION_POINTER_DOWN, 1, listOf(chip, finger)))
        assertEquals(Event(Touch.ACTION_MOVE, 0, listOf(finger)), filter.filter(Touch.ACTION_MOVE, 0, listOf(chip, finger)))
        assertEquals(Event(Touch.ACTION_UP, 0, listOf(finger)), filter.filter(Touch.ACTION_POINTER_UP, 1, listOf(chip, finger)))
        assertNull(filter.filter(Touch.ACTION_UP, 0, listOf(chip)))
    }

    @Test
    fun indicesFollowTheForwardedPointersOnly() {
        val c = p(2, 900f, 300f)
        filter.filter(Touch.ACTION_DOWN, 0, listOf(a))
        filter.filter(Touch.ACTION_POINTER_DOWN, 1, listOf(a, atEdge))
        assertEquals(
            Event(Touch.ACTION_POINTER_DOWN, 1, listOf(a, c)),
            filter.filter(Touch.ACTION_POINTER_DOWN, 2, listOf(a, atEdge, c)),
        )
        // Through the translator, the action names the right pointer id.
        val e = filter.filter(Touch.ACTION_POINTER_UP, 2, listOf(a, atEdge, c))!!
        val touch = TouchTranslator.translate(e.actionMasked, e.actionIndex, 10, e.pointers, VideoRect(0, 0, 2400, 1080))!!
        assertEquals(Touch.ACTION_POINTER_UP, touch.action)
        assertEquals(2, touch.actionPointerId)
        assertEquals(listOf(0, 2), touch.pointers.map { it.id })
    }

    @Test
    fun cancelOnlyConcernsForwardedPointersAndEverythingResetsOnDown() {
        filter.filter(Touch.ACTION_DOWN, 0, listOf(a))
        filter.filter(Touch.ACTION_POINTER_DOWN, 1, listOf(a, onButton))
        assertEquals(Event(Touch.ACTION_CANCEL, 0, listOf(a)), filter.filter(Touch.ACTION_CANCEL, 0, listOf(a, onButton)))
        assertNull(filter.filter(Touch.ACTION_MOVE, 0, listOf(a)))
        // A missed UP never leaves a pointer held back into the next gesture.
        filter.filter(Touch.ACTION_DOWN, 0, listOf(a))
        filter.filter(Touch.ACTION_POINTER_DOWN, 1, listOf(a, onButton))
        assertEquals(Event(Touch.ACTION_DOWN, 0, listOf(b)), filter.filter(Touch.ACTION_DOWN, 0, listOf(b)))
        assertEquals(emptySet<Int>(), filter.heldBack)
    }
}
