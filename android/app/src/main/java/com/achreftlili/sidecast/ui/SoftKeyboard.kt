package com.achreftlili.sidecast.ui

import android.os.Build
import android.view.inputmethod.InputMethodManager
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.achreftlili.sidecast.input.KeyboardInputView

/**
 * Shows / hides the soft keyboard for [input]. Visibility is tracked by [ImeVisibility]; a tap on
 * the button decides on the keyboard's visibility queried at that moment. [onHidden] runs whenever
 * the keyboard is believed to have closed.
 */
internal class SoftKeyboard(
    private val input: KeyboardInputView,
    private val onHidden: () -> Unit,
) {
    private val imm: InputMethodManager = checkNotNull(input.context.getSystemService(InputMethodManager::class.java))
    private val visibility = ImeVisibility()

    val isShown: Boolean get() = visibility.shown

    fun toggle() {
        if (visibility.current(liveImeInsets())) hide() else show()
    }

    fun show() {
        if (!input.isFocused) input.requestFocus()
        imm.restartInput(input) // fresh IME session: empty context, nothing stale to edit
        if (!imm.showSoftInput(input, 0)) input.post { imm.showSoftInput(input, 0) }
        visibility.showRequested()
    }

    fun hide() {
        imm.hideSoftInputFromWindow(input.windowToken, 0)
        if (visibility.hidden()) onHidden()
    }

    /** Called with every WindowInsets dispatch on API 30+. */
    fun onImeInsets(visible: Boolean) {
        if (visibility.onInsets(visible)) onHidden()
    }

    /** BACK reached the IME while it may be up: it closes itself. */
    fun onBackToIme() {
        if (visibility.hidden()) onHidden()
    }

    private fun liveImeInsets(): Boolean? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            ViewCompat.getRootWindowInsets(input)?.isVisible(WindowInsetsCompat.Type.ime())
        } else {
            null // the compat answer is a heuristic that adjustNothing defeats
        }
}
