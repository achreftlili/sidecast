package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.protocol.Key

/**
 * Turns InputConnection calls into [KeyboardEvent]s for the Mac. Pure Kotlin (no Android types)
 * so the IME protocol handling is unit tested.
 *
 * Model: the phone-side field holds only the last committed character (so the keyboard spaces and
 * capitalises dictation correctly) and the IME's composing region. Composed text is
 * typed on the Mac immediately (voice typing streams partial results this way); when the
 * composition changes, only the difference is sent — one Backspace per character that changed,
 * then the new text — so the Mac always holds exactly what the IME shows. Autocorrect
 * (`commitText` replacing the composition) is handled the same way. Text committed earlier than
 * the last character is never shown to the IME again, so it cannot try to rewrite what is already
 * on the Mac; deleting the visible character is a plain Backspace.
 *
 * The translator also mirrors what the IME *believes* precedes the cursor ([cursor], and the
 * recently committed characters) for two things only: converting `deleteSurroundingText` lengths
 * (UTF-16 units: an emoji is two or more) into one Backspace per character, and letting an IME
 * re-open the word it just committed with `setComposingRegion`.
 *
 * Not thread-safe: InputConnection calls arrive on the UI thread.
 */
class ImeTextTranslator(
    private val historyLimit: Int = 256,
    private val maxDeletePresses: Int = MAX_DELETE_PRESSES,
) {
    private var composing: List<TypedUnit> = emptyList()
    private var isComposing = false
    private var composingStart = 0
    private val history = ArrayDeque<TypedUnit>()

    /** The IME's current composing text, as it wrote it ("" when not composing). */
    var composingText: String = ""
        private set

    /** Where the IME believes the cursor is (UTF-16 offset since the input session started). */
    var cursor: Int = 0
        private set

    /** New input session (onCreateInputConnection / restartInput): the IME starts over too. */
    fun reset() {
        endComposition()
        history.clear()
        cursor = 0
    }

    /** commitText: replace the composition (if any) with [text] and finish it. */
    fun commitText(text: CharSequence): List<KeyboardEvent> {
        val units = TypedText.segment(text)
        val events = TypedText.diff(composing, units)
        val start = if (isComposing) composingStart else cursor
        endComposition()
        cursor = start + text.length
        remember(units)
        return events
    }

    /** setComposingText: replace the composition (or start one at the cursor) with [text]. */
    fun setComposingText(text: CharSequence): List<KeyboardEvent> {
        if (!isComposing) {
            isComposing = true
            composingStart = cursor
        }
        val units = TypedText.segment(text)
        val events = TypedText.diff(composing, units)
        composing = units
        composingText = text.toString()
        cursor = composingStart + text.length
        return events
    }

    /** finishComposingText: the composition is already on the Mac; just stop tracking it. */
    fun finishComposingText(): List<KeyboardEvent> {
        if (isComposing) {
            remember(composing)
            endComposition()
        }
        return emptyList()
    }

    /**
     * setComposingRegion: the IME re-opens text before the cursor as a composition (e.g. the word
     * it just committed, after a Backspace). Honoured only when [start]..[end] ends at the cursor
     * and covers whole characters the IME committed; otherwise there is simply no composition.
     */
    fun setComposingRegion(start: Int, end: Int): List<KeyboardEvent> {
        finishComposingText()
        val from = minOf(start, end)
        val to = maxOf(start, end)
        if (from == to || to != cursor || from < 0) return emptyList()
        val adopted = takeFromHistory(to - from) ?: return emptyList()
        isComposing = true
        composingStart = from
        composing = adopted
        composingText = adopted.joinToString("") { it.raw }
        return emptyList()
    }

    /** replaceText (API 34): [start]..[end] becomes [text], cursor after it. */
    fun replaceText(start: Int, end: Int, text: CharSequence): List<KeyboardEvent> {
        val from = minOf(start, end)
        val to = maxOf(start, end)
        if (from == to || to != cursor) {
            finishComposingText()
            return commitText(text)
        }
        setComposingRegion(from, to)
        if (isComposing) return commitText(text)
        // The region is not text the IME committed through us: delete it blindly, then type.
        return deleteSurroundingText(to - from, 0) + commitText(text)
    }

    /** deleteSurroundingText: lengths in UTF-16 units. */
    fun deleteSurroundingText(before: Int, after: Int): List<KeyboardEvent> =
        delete(before, after) { it.length }

    /** deleteSurroundingTextInCodePoints: lengths in code points. */
    fun deleteSurroundingTextInCodePoints(before: Int, after: Int): List<KeyboardEvent> =
        delete(before, after) { it.codePointCount(0, it.length) }

    /**
     * A key typed outside the IME's text (hardware keyboard, IME key event, editor action). The Mac
     * changed in a way the IME's model does not reflect: keep only what is still certain.
     */
    fun onKeyPressed(event: KeyboardEvent) {
        finishComposingText()
        if (event is KeyboardEvent.Press && event.key == Key.BACKSPACE && event.modifiers == 0) {
            history.removeLastOrNull()
        } else {
            history.clear()
        }
    }

    /**
     * The Mac's cursor may have moved (the user touched the stream): nothing before it is known
     * any more, and the composition must not be continued at the new place. Returns true when
     * there was a context or composition to drop (the IME should then restart its session).
     */
    fun forgetContext(): Boolean {
        val had = isComposing || history.isNotEmpty()
        endComposition()
        history.clear()
        return had
    }

    /**
     * InputConnection.getTextBeforeCursor: the last committed character (one user-perceived
     * character, so an emoji is never cut in half; nothing after a forgotten context) followed by
     * the composition, at most [maxChars] UTF-16 units without splitting a surrogate pair.
     */
    fun textBeforeCursor(maxChars: Int): String {
        if (maxChars <= 0) return ""
        val context = history.lastOrNull()?.takeIf { it.output != null }?.raw ?: ""
        val text = context + composingText
        if (text.length <= maxChars) return text
        var start = text.length - maxChars
        if (Character.isLowSurrogate(text[start]) && Character.isHighSurrogate(text[start - 1])) start++
        return text.substring(start)
    }

    private fun delete(before: Int, after: Int, measure: (String) -> Int): List<KeyboardEvent> {
        finishComposingText()
        var remaining = before.coerceAtLeast(0)
        var presses = 0
        var removedChars = 0
        while (remaining > 0) {
            val unit = history.removeLastOrNull() ?: break
            remaining -= measure(unit.raw)
            removedChars += unit.raw.length
            if (unit.output != null) presses++
        }
        if (remaining > 0) {
            // Beyond what the IME committed through us: one Backspace per unit it asked for.
            presses += remaining
            removedChars += remaining
        }
        cursor = (cursor - removedChars).coerceAtLeast(0)
        val backspaces = presses.coerceAtMost(maxDeletePresses)
        val forwardDeletes = after.coerceIn(0, maxDeletePresses)
        return List(backspaces) { KeyboardEvent.Press(Key.BACKSPACE) } +
            List(forwardDeletes) { KeyboardEvent.Press(Key.FORWARD_DELETE) }
    }

    /** Removes the last units covering exactly [chars] UTF-16 units, or returns null untouched. */
    private fun takeFromHistory(chars: Int): List<TypedUnit>? {
        val taken = ArrayList<TypedUnit>()
        var need = chars
        while (need > 0) {
            val unit = history.removeLastOrNull() ?: break
            taken += unit
            need -= unit.raw.length
        }
        if (need != 0) {
            for (k in taken.indices.reversed()) history.addLast(taken[k])
            return null
        }
        taken.reverse()
        return taken
    }

    private fun remember(units: List<TypedUnit>) {
        for (u in units) history.addLast(u)
        while (history.size > historyLimit) history.removeFirstOrNull()
    }

    private fun endComposition() {
        isComposing = false
        composing = emptyList()
        composingText = ""
    }

    companion object {
        /** Upper bound of Backspaces / forward deletes for one deleteSurroundingText call. */
        const val MAX_DELETE_PRESSES = 64
    }
}
