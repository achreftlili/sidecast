package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.protocol.Key
import java.text.Normalizer

/**
 * What the phone's keyboard does on the Mac (§7.1). Produced by [ImeTextTranslator] and
 * [KeyTranslator]; sent as TEXT / KEY.
 */
sealed interface KeyboardEvent {
    /** Text to type. Never contains control characters (those are [Press]es). */
    data class Type(val text: String) : KeyboardEvent

    /** A [Key] constant pressed with `Key.MOD_*` [modifiers] held. */
    data class Press(val key: Int, val modifiers: Int = 0) : KeyboardEvent
}

/**
 * One user-perceived character: [raw] as the IME wrote it (UTF-16) and [output], what the Mac
 * receives for it (null for a dropped control character). One non-null unit = one Backspace on
 * the Mac.
 */
internal class TypedUnit(val raw: String, val output: KeyboardEvent?)

/** Splits IME text into [TypedUnit]s and computes the keystrokes between two versions of it. */
internal object TypedText {
    private const val ZWJ = 0x200D

    /**
     * Segments [text] into approximate grapheme clusters (base + combining marks, emoji modifier /
     * ZWJ / variation-selector / tag sequences, flag pairs). Each cluster is NFC-normalized for the
     * Mac. "\n", "\r", "\r\n" become Return and "\t" Tab; other control characters are dropped.
     */
    fun segment(text: CharSequence): List<TypedUnit> {
        val s = text.toString()
        val units = ArrayList<TypedUnit>(s.length)
        var i = 0
        while (i < s.length) {
            val start = i
            val cp = s.codePointAt(i)
            i += Character.charCount(cp)
            if (Character.isISOControl(cp)) {
                if (cp == '\r'.code && i < s.length && s[i] == '\n') i++
                val output = when (cp) {
                    '\n'.code, '\r'.code -> KeyboardEvent.Press(Key.RETURN)
                    '\t'.code -> KeyboardEvent.Press(Key.TAB)
                    else -> null
                }
                units += TypedUnit(s.substring(start, i), output)
                continue
            }
            i = clusterEnd(s, i, cp)
            val raw = s.substring(start, i)
            units += TypedUnit(raw, KeyboardEvent.Type(Normalizer.normalize(raw, Normalizer.Form.NFC)))
        }
        return units
    }

    /**
     * Keystrokes that turn [old] (already on the Mac, just before the cursor) into [new]: one
     * Backspace per unit after their common prefix, then the rest of [new].
     */
    fun diff(old: List<TypedUnit>, new: List<TypedUnit>): List<KeyboardEvent> {
        var same = 0
        val limit = minOf(old.size, new.size)
        while (same < limit && old[same].output == new[same].output) same++
        val events = ArrayList<KeyboardEvent>()
        for (k in same until old.size) {
            if (old[k].output != null) events += KeyboardEvent.Press(Key.BACKSPACE)
        }
        val run = StringBuilder()
        for (k in same until new.size) {
            when (val output = new[k].output) {
                is KeyboardEvent.Type -> run.append(output.text)
                is KeyboardEvent.Press -> {
                    if (run.isNotEmpty()) events += KeyboardEvent.Type(run.toString())
                    run.setLength(0)
                    events += output
                }
                null -> Unit
            }
        }
        if (run.isNotEmpty()) events += KeyboardEvent.Type(run.toString())
        return events
    }

    /** End (exclusive) of the cluster that starts with [first]; [from] is just after [first]. */
    private fun clusterEnd(s: String, from: Int, first: Int): Int {
        var end = from
        var previous = first
        var regionalIndicators = if (isRegionalIndicator(first)) 1 else 0
        while (end < s.length) {
            val cp = s.codePointAt(end)
            val joins = when {
                Character.isISOControl(cp) -> false
                previous == ZWJ -> true // ZWJ sequence, e.g. a family emoji
                cp == ZWJ || isExtend(cp) -> true
                regionalIndicators == 1 && isRegionalIndicator(cp) -> true // second half of a flag
                else -> false
            }
            if (!joins) break
            if (isRegionalIndicator(cp)) regionalIndicators++
            previous = cp
            end += Character.charCount(cp)
        }
        return end
    }

    private fun isExtend(cp: Int): Boolean = when (Character.getType(cp).toByte()) {
        Character.NON_SPACING_MARK, Character.ENCLOSING_MARK, Character.COMBINING_SPACING_MARK -> true
        else ->
            cp in 0xFE00..0xFE0F || // variation selectors (emoji / text presentation)
                cp in 0x1F3FB..0x1F3FF || // emoji skin-tone modifiers
                cp in 0xE0020..0xE007F || // tag sequences (subdivision flags)
                cp == 0x200C || // ZWNJ
                cp in 0x1160..0x11FF || // conjoining Hangul vowels / final consonants
                cp in 0xD7B0..0xD7FF
    }

    private fun isRegionalIndicator(cp: Int): Boolean = cp in 0x1F1E6..0x1F1FF
}
