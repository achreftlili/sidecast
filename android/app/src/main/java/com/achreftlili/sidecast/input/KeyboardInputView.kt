package com.achreftlili.sidecast.input

import android.content.Context
import android.text.InputType
import android.util.AttributeSet
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager

/**
 * Invisible, focusable text editor that the soft keyboard types into. It draws nothing; its
 * InputConnection (from [bridge]) forwards everything to the Mac.
 */
class KeyboardInputView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : View(context, attrs) {
    var bridge: KeyboardBridge? = null

    /** BACK reached this view before the IME: the soft keyboard is about to close. */
    var onBackBeforeIme: (() -> Unit)? = null

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        defaultFocusHighlightEnabled = false
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
        importantForAutofill = IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
    }

    override fun onCheckIsTextEditor(): Boolean = true

    /**
     * The user touched the stream, so the Mac's cursor may have moved: forget the typing context
     * and, if there was any, restart the IME session so it cannot continue a word somewhere else.
     */
    fun onRemoteCursorMayHaveMoved() {
        if (bridge?.forgetContext() != true) return
        val imm = context.getSystemService(InputMethodManager::class.java) ?: return
        if (imm.isActive(this)) imm.restartInput(this)
    }

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection? {
        val b = bridge ?: return null
        outAttrs.inputType = INPUT_TYPE
        outAttrs.imeOptions = IME_OPTIONS
        outAttrs.initialSelStart = 0
        outAttrs.initialSelEnd = 0
        return b.newInputConnection(this)
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        bridge?.onKeyDown(event) == true || super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
        bridge?.onKeyUp(event) == true || super.onKeyUp(keyCode, event)

    override fun onKeyPreIme(keyCode: Int, event: KeyEvent): Boolean {
        // Observe only: the IME still gets BACK and hides itself.
        if (keyCode == KeyEvent.KEYCODE_BACK && event.action == KeyEvent.ACTION_UP) onBackBeforeIme?.invoke()
        return super.onKeyPreIme(keyCode, event)
    }

    companion object {
        /**
         * Normal text so the keyboard keeps suggestions, autocorrect and voice input (dictation);
         * [ImeTextTranslator]'s composing deltas keep the Mac in sync. For strict per-character
         * typing (no composing at all) use instead:
         * `TYPE_CLASS_TEXT or TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or TYPE_TEXT_FLAG_NO_SUGGESTIONS`
         * (most keyboards then hide their microphone key).
         */
        const val INPUT_TYPE =
            InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE or InputType.TYPE_TEXT_FLAG_AUTO_CORRECT

        /**
         * No fullscreen / extract UI in landscape (the stream stays visible above the keyboard), and
         * no personalized learning: whatever is typed on the Mac, passwords included, must not end
         * up in the keyboard's dictionary.
         */
        const val IME_OPTIONS =
            EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_FULLSCREEN or
                EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING or EditorInfo.IME_ACTION_NONE
    }
}
