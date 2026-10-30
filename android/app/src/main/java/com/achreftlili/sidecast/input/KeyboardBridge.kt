package com.achreftlili.sidecast.input

import android.os.Build
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.ExtractedText
import android.view.inputmethod.ExtractedTextRequest
import android.view.inputmethod.InputConnection
import android.view.inputmethod.SurroundingText
import android.view.inputmethod.TextAttribute
import android.view.inputmethod.TextSnapshot
import androidx.annotation.RequiresApi
import com.achreftlili.sidecast.protocol.Key

/**
 * Phone keyboard -> Mac (§7.1). Owns the [ImeTextTranslator] behind the [KeyboardInputView]'s
 * InputConnection and handles key events (hardware keyboard, or keys an IME sends) through
 * [KeyTranslator]. Every resulting [KeyboardEvent] goes to [send], on the UI thread.
 */
class KeyboardBridge(private val send: (KeyboardEvent) -> Unit) {
    private val translator = ImeTextTranslator()
    private val consumedKeyCodes = HashSet<Int>()

    /**
     * True only while streaming. When false nothing is sent and key events keep their normal
     * Android meaning (focus navigation, overlay buttons).
     */
    var enabled = false

    internal fun newInputConnection(view: View): InputConnection {
        translator.reset() // a new connection is a new IME session: both sides start empty
        return KeyboardInputConnection(view, this, translator)
    }

    /** See [ImeTextTranslator.forgetContext]. True when the IME session should be restarted. */
    fun forgetContext(): Boolean = translator.forgetContext()

    /** Key down from the focused view, the activity, or the IME. True when consumed. */
    fun onKeyDown(event: KeyEvent): Boolean {
        if (!enabled) return false
        val output = KeyTranslator.translate(event.keyCode, event.metaState, event.getUnicodeChar(event.metaState))
            ?: return false
        consumedKeyCodes += event.keyCode
        translator.onKeyPressed(output)
        send(output)
        return true
    }

    /** Consumes the up of every down that [onKeyDown] consumed (nothing is sent for ups). */
    fun onKeyUp(event: KeyEvent): Boolean = consumedKeyCodes.remove(event.keyCode)

    /** The IME pressed its action / Enter key as an editor action. */
    internal fun onEditorAction() {
        val press = KeyboardEvent.Press(Key.RETURN)
        translator.onKeyPressed(press)
        emit(listOf(press))
    }

    internal fun emit(events: List<KeyboardEvent>) {
        if (enabled) events.forEach(send)
    }
}

/**
 * The IME's view of an almost empty field (the last committed character, then the current
 * composition; see [ImeTextTranslator.textBeforeCursor]), with every edit forwarded to the Mac. `fullEditor = false`: there is no real Editable behind it, and the
 * base class's text-to-key-event fallback is never reached because every editing call is
 * overridden.
 */
internal class KeyboardInputConnection(
    view: View,
    private val bridge: KeyboardBridge,
    private val translator: ImeTextTranslator,
) : BaseInputConnection(view, false) {
    private var batchDepth = 0

    override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
        bridge.emit(translator.commitText(text ?: ""))
        return true
    }

    override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
        bridge.emit(translator.setComposingText(text ?: ""))
        return true
    }

    override fun finishComposingText(): Boolean {
        bridge.emit(translator.finishComposingText())
        return true
    }

    override fun setComposingRegion(start: Int, end: Int): Boolean {
        bridge.emit(translator.setComposingRegion(start, end))
        return true
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    override fun replaceText(
        start: Int,
        end: Int,
        text: CharSequence,
        newCursorPosition: Int,
        textAttribute: TextAttribute?,
    ): Boolean {
        bridge.emit(translator.replaceText(start, end, text))
        return true
    }

    override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
        if (beforeLength < 0 || afterLength < 0) return false
        bridge.emit(translator.deleteSurroundingText(beforeLength, afterLength))
        return true
    }

    override fun deleteSurroundingTextInCodePoints(beforeLength: Int, afterLength: Int): Boolean {
        if (beforeLength < 0 || afterLength < 0) return false
        bridge.emit(translator.deleteSurroundingTextInCodePoints(beforeLength, afterLength))
        return true
    }

    /**
     * Many IMEs send Backspace / Enter / arrows as key events. Mapped keys are handled here
     * directly; anything else (e.g. BACK) is dispatched normally by the base class.
     */
    override fun sendKeyEvent(event: KeyEvent): Boolean {
        val handled = when (event.action) {
            KeyEvent.ACTION_DOWN -> bridge.onKeyDown(event)
            KeyEvent.ACTION_UP -> bridge.onKeyUp(event)
            else -> false
        }
        return handled || super.sendKeyEvent(event)
    }

    override fun performEditorAction(editorAction: Int): Boolean {
        bridge.onEditorAction()
        return true
    }

    /** The cursor on the Mac is not visible from here; moving it locally means nothing. */
    override fun setSelection(start: Int, end: Int): Boolean = true

    override fun getTextBeforeCursor(n: Int, flags: Int): CharSequence = translator.textBeforeCursor(n)

    override fun getTextAfterCursor(n: Int, flags: Int): CharSequence = ""

    override fun getSelectedText(flags: Int): CharSequence? = null

    /** No auto-capitalization: the text around the Mac's cursor is unknown. */
    override fun getCursorCapsMode(reqModes: Int): Int = 0

    override fun getExtractedText(request: ExtractedTextRequest?, flags: Int): ExtractedText? = null

    @RequiresApi(Build.VERSION_CODES.S)
    override fun getSurroundingText(beforeLength: Int, afterLength: Int, flags: Int): SurroundingText {
        val before = translator.textBeforeCursor(beforeLength)
        return SurroundingText(before, before.length, before.length, (translator.cursor - before.length).coerceAtLeast(0))
    }

    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    override fun takeSnapshot(): TextSnapshot? = null

    override fun beginBatchEdit(): Boolean {
        batchDepth++
        return true
    }

    override fun endBatchEdit(): Boolean {
        if (batchDepth > 0) batchDepth--
        return batchDepth > 0
    }
}
