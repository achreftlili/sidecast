package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.protocol.Touch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TouchTranslatorTest {
    private val rect = VideoRect(0, 0, 1000, 500)
    private val eps = 1e-5f

    private fun p(id: Int, x: Float, y: Float) = PointerSample(id, x, y)

    @Test
    fun downIsNormalized() {
        val t = TouchTranslator.translate(Touch.ACTION_DOWN, 0, 1234, listOf(p(0, 500f, 125f)), rect)!!
        assertEquals(Touch.ACTION_DOWN, t.action)
        assertEquals(0, t.actionPointerId)
        assertEquals(1234L, t.eventTimeMs)
        assertEquals(0.5f, t.pointers[0].x, eps)
        assertEquals(0.25f, t.pointers[0].y, eps)
    }

    @Test
    fun pointerDownAndUpReferToActionIndex() {
        val pointers = listOf(p(4, 0f, 0f), p(7, 1000f, 500f))
        val down = TouchTranslator.translate(Touch.ACTION_POINTER_DOWN, 1, 0, pointers, rect)!!
        assertEquals(7, down.actionPointerId)
        assertEquals(2, down.pointers.size)
        val up = TouchTranslator.translate(Touch.ACTION_POINTER_UP, 0, 0, pointers, rect)!!
        assertEquals(4, up.actionPointerId)
        // The lifting pointer is still in the list (§7).
        assertTrue(up.pointers.any { it.id == 4 })
    }

    @Test
    fun moveAndCancelUseFirstPointer() {
        val pointers = listOf(p(2, 10f, 10f), p(0, 20f, 20f))
        assertEquals(2, TouchTranslator.translate(Touch.ACTION_MOVE, 1, 0, pointers, rect)!!.actionPointerId)
        assertEquals(2, TouchTranslator.translate(Touch.ACTION_CANCEL, 0, 0, pointers, rect)!!.actionPointerId)
    }

    @Test
    fun capsAtTenPointers() {
        val pointers = List(12) { p(it, it * 10f, 0f) }
        val move = TouchTranslator.translate(Touch.ACTION_MOVE, 0, 0, pointers, rect)!!
        assertEquals(10, move.pointers.size)
        // A DOWN for the 11th finger is not representable: dropped entirely.
        assertNull(TouchTranslator.translate(Touch.ACTION_POINTER_DOWN, 10, 0, pointers, rect))
    }

    @Test
    fun ignoresNonTouchActions() {
        for (action in listOf(4 /* OUTSIDE */, 7 /* HOVER_MOVE */, 8 /* SCROLL */, 9, 10)) {
            assertNull(TouchTranslator.translate(action, 0, 0, listOf(p(0, 0f, 0f)), rect))
        }
        assertNull(TouchTranslator.translate(Touch.ACTION_MOVE, 0, 0, emptyList(), rect))
    }

    @Test
    fun eventTimeWrapsToU32() {
        val t = TouchTranslator.translate(Touch.ACTION_DOWN, 0, 0x1_0000_0010L, listOf(p(0, 0f, 0f)), rect)!!
        assertEquals(0x10L, t.eventTimeMs)
    }

    @Test
    fun letterboxOffsetIsRemoved() {
        val boxed = Letterbox.fit(2400, 1080, 1920, 1080)
        val t = TouchTranslator.translate(Touch.ACTION_DOWN, 0, 0, listOf(p(0, 240f, 1080f)), boxed)!!
        assertEquals(0f, t.pointers[0].x, eps)
        assertEquals(1f, t.pointers[0].y, eps)
    }

    @Test
    fun threeFingerTapIsDetected() {
        val detector = ThreeFingerTapDetector(slopPx = 10f)
        val one = listOf(p(0, 100f, 100f))
        val two = one + p(1, 200f, 100f)
        val three = two + p(2, 300f, 100f)
        assertFalse(detector.onEvent(Touch.ACTION_DOWN, 0, one))
        assertFalse(detector.onEvent(Touch.ACTION_POINTER_DOWN, 20, two))
        assertFalse(detector.onEvent(Touch.ACTION_POINTER_DOWN, 30, three))
        assertFalse(detector.onEvent(Touch.ACTION_MOVE, 60, three.map { it.copy(x = it.x + 3f) }))
        assertFalse(detector.onEvent(Touch.ACTION_POINTER_UP, 150, three))
        assertFalse(detector.onEvent(Touch.ACTION_POINTER_UP, 160, two))
        assertTrue(detector.onEvent(Touch.ACTION_UP, 170, one))
    }

    @Test
    fun threeFingerDragOrSlowTapIsNotATap() {
        val detector = ThreeFingerTapDetector(slopPx = 10f)
        val three = listOf(p(0, 0f, 0f), p(1, 50f, 0f), p(2, 100f, 0f))
        detector.onEvent(Touch.ACTION_DOWN, 0, three.take(1))
        detector.onEvent(Touch.ACTION_POINTER_DOWN, 10, three)
        detector.onEvent(Touch.ACTION_MOVE, 50, three.map { it.copy(y = 40f) })
        assertFalse(detector.onEvent(Touch.ACTION_UP, 100, three.take(1)))

        detector.onEvent(Touch.ACTION_DOWN, 1000, three.take(1))
        detector.onEvent(Touch.ACTION_POINTER_DOWN, 1010, three)
        assertFalse(detector.onEvent(Touch.ACTION_UP, 2000, three.take(1)))

        // Two fingers never count.
        detector.onEvent(Touch.ACTION_DOWN, 3000, three.take(1))
        detector.onEvent(Touch.ACTION_POINTER_DOWN, 3010, three.take(2))
        assertFalse(detector.onEvent(Touch.ACTION_UP, 3050, three.take(1)))
    }
}
