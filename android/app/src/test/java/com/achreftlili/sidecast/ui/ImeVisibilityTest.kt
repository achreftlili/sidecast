package com.achreftlili.sidecast.ui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The keyboard button must open and close the keyboard reliably, with or without IME insets. */
class ImeVisibilityTest {
    private val v = ImeVisibility()

    /** One tap: returns true when it hides, false when it shows (as SoftKeyboard.toggle does). */
    private fun tap(insets: Boolean?): Boolean {
        val shown = v.current(insets)
        if (shown) v.hidden() else v.showRequested()
        return shown
    }

    @Test
    fun withoutInsetsTapsAlternate() {
        // API 29, or insets never report the IME with adjustNothing: own state only.
        assertFalse(tap(insets = null)) // show
        assertTrue(tap(insets = null)) // hide
        assertFalse(tap(insets = false)) // show: "false" is not trusted until insets proved reliable
        assertTrue(tap(insets = false)) // hide
    }

    @Test
    fun backToTheImeMeansTheNextTapOpens() {
        tap(insets = null)
        assertTrue(v.hidden()) // BACK reached the IME: it closed, onHidden runs
        assertFalse(v.hidden()) // only once
        assertFalse(tap(insets = null))
        assertTrue(v.shown)
    }

    @Test
    fun reliableInsetsDecideAtTapTime() {
        tap(insets = null) // show
        assertFalse(v.onInsets(visible = true)) // the platform reports the IME: insets now trusted
        // The keyboard closed itself (its own hide key) and no dispatch reached us:
        // the live query at tap time still opens it with one tap.
        assertFalse(tap(insets = false))
        // And a keyboard opened by the system is closed by one tap.
        assertTrue(tap(insets = true))
    }

    @Test
    fun insetsDispatchClosingTheKeyboardIsReportedOnce() {
        assertFalse("not trusted yet", v.onInsets(visible = false))
        v.showRequested()
        assertFalse(v.onInsets(visible = true))
        assertTrue(v.onInsets(visible = false)) // closed: re-hide system bars
        assertFalse(v.onInsets(visible = false))
        assertFalse(v.shown)
    }

    @Test
    fun keyboardOpenedWithoutUsIsKnown() {
        assertFalse(v.onInsets(visible = true))
        assertTrue(v.shown)
        assertTrue(tap(insets = true)) // hides
    }
}
