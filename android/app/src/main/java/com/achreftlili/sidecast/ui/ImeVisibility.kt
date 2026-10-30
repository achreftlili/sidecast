package com.achreftlili.sidecast.ui

/**
 * Best knowledge of whether the soft keyboard is up, for the keyboard button. Pure, so the toggle
 * behaviour is unit tested.
 *
 * The platform's IME insets (API 30+) are authoritative once they have been seen reporting the
 * keyboard as visible for this window; with adjustNothing that is likely but not guaranteed. Until
 * then the state follows our own show/hide requests and BACK reaching the IME. Without insets, a
 * keyboard that closes itself unnoticed (its own hide key) costs one extra tap.
 */
internal class ImeVisibility {
    private var insetsReliable = false

    var shown = false
        private set

    /** At tap time. [insetsVisible] is the live WindowInsets answer, null when unavailable. */
    fun current(insetsVisible: Boolean?): Boolean {
        if (insetsVisible == true) {
            insetsReliable = true
            shown = true
        } else if (insetsVisible == false && insetsReliable) {
            shown = false
        }
        return shown
    }

    fun showRequested() {
        shown = true
    }

    /** Hide requested, or BACK went to the IME. True when this closed a shown keyboard. */
    fun hidden(): Boolean {
        if (!shown) return false
        shown = false
        return true
    }

    /** Every WindowInsets dispatch (API 30+). True when it reports a shown keyboard closing. */
    fun onInsets(visible: Boolean): Boolean {
        if (visible) {
            insetsReliable = true
            shown = true
            return false
        }
        return insetsReliable && hidden()
    }
}
