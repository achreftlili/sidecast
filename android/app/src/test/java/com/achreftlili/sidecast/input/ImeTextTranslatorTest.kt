package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.input.KeyboardEvent.Press
import com.achreftlili.sidecast.input.KeyboardEvent.Type
import com.achreftlili.sidecast.protocol.Key
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** What the Mac's focused text field ends up holding, given the events the phone sent. */
private class FakeMacField {
    val text = StringBuilder()

    fun apply(events: List<KeyboardEvent>): List<KeyboardEvent> {
        for (e in events) {
            when (e) {
                is Type -> {
                    assertTrue("control character in TEXT: $e", e.text.none { Character.isISOControl(it) })
                    text.append(e.text)
                }
                is Press -> when (e.key) {
                    // Backspace deletes one user-perceived character, as on macOS.
                    Key.BACKSPACE -> TypedText.segment(text).lastOrNull()?.let { text.setLength(text.length - it.raw.length) }
                    Key.RETURN -> text.append('\n')
                    Key.TAB -> text.append('\t')
                    else -> fail("unexpected key $e")
                }
            }
        }
        return events
    }

    override fun toString() = text.toString()
}

class ImeTextTranslatorTest {
    private val ime = ImeTextTranslator()
    private val mac = FakeMacField()

    private fun backspaces(n: Int) = List(n) { Press(Key.BACKSPACE) }

    private fun commit(text: String) = mac.apply(ime.commitText(text))

    private fun compose(text: String) = mac.apply(ime.setComposingText(text))

    private fun finish() = mac.apply(ime.finishComposingText())

    private fun delete(before: Int, after: Int = 0) = mac.apply(ime.deleteSurroundingText(before, after))

    // ------------------------------------------------------------------ commit

    @Test
    fun committedTextIsTyped() {
        assertEquals(listOf(Type("a")), commit("a"))
        assertEquals(listOf(Type("bc d")), commit("bc d"))
        assertEquals("abc d", mac.toString())
    }

    @Test
    fun newlineAndTabBecomeKeys() {
        assertEquals(listOf(Press(Key.RETURN)), commit("\n"))
        assertEquals(listOf(Press(Key.TAB)), commit("\t"))
        assertEquals(listOf(Press(Key.RETURN)), commit("\r\n"))
        assertEquals(listOf(Press(Key.RETURN)), commit("\r"))
        assertEquals(
            listOf(Type("a"), Press(Key.RETURN), Type("b"), Press(Key.TAB), Type("c")),
            commit("a\nb\tc"),
        )
    }

    @Test
    fun otherControlCharactersAreDropped() {
        assertEquals(listOf(Type("abc")), commit("a\u0000b\u007Fc\u0085"))
        assertEquals(emptyList<KeyboardEvent>(), commit("\u001B"))
    }

    @Test
    fun textIsNfcNormalized() {
        assertEquals(listOf(Type("é")), commit("e\u0301"))
    }

    // ------------------------------------------------------------------ composing

    @Test
    fun voiceTypingStreamsOnlyTheNewText() {
        assertEquals(listOf(Type("hel")), compose("hel"))
        assertEquals(listOf(Type("lo")), compose("hello"))
        assertEquals(listOf(Type(" wor")), compose("hello wor"))
        assertEquals(listOf(Type("ld")), compose("hello world"))
        assertEquals(emptyList<KeyboardEvent>(), finish())
        assertEquals("hello world", mac.toString())
        assertEquals("d", ime.textBeforeCursor(100)) // only the last character is visible
    }

    @Test
    fun voiceTypingRevisionsSendBackspacesThenTheDifference() {
        compose("I scream")
        assertEquals(backspaces(7) + Type("ce cream"), compose("Ice cream"))
        assertEquals(backspaces(5) + Type("team"), compose("Ice team"))
        assertEquals(listOf(Type(".")), commit("Ice team."))
        assertEquals("Ice team.", mac.toString())
    }

    @Test
    fun autocorrectCommitReplacesTheComposition() {
        compose("t")
        compose("te")
        compose("teh")
        assertEquals(backspaces(2) + Type("he "), commit("the "))
        assertEquals("the ", mac.toString())
        assertEquals(" ", ime.textBeforeCursor(10))

        // The next word composes after it, independently.
        compose("c")
        compose("ca")
        commit("cat")
        assertEquals("the cat", mac.toString())
    }

    @Test
    fun finishedCompositionIsNeverEditedAgain() {
        compose("foo")
        finish()
        assertEquals(listOf(Type("b")), compose("b"))
        compose("bar")
        commit("bar")
        assertEquals("foobar", mac.toString())
    }

    @Test
    fun emptyCompositionOrCommitDeletesTheComposition() {
        compose("abc")
        assertEquals(backspaces(3), compose(""))
        compose("xy")
        assertEquals(backspaces(2), commit(""))
        assertEquals("", mac.toString())
    }

    @Test
    fun imeSeesOnlyTheLastCommittedCharacterAndTheComposition() {
        commit("hello ")
        assertEquals(" ", ime.textBeforeCursor(100))
        compose("wor")
        assertEquals(" wor", ime.textBeforeCursor(100))
        assertEquals("or", ime.textBeforeCursor(2))
        assertEquals("", ime.textBeforeCursor(0))
        assertEquals(9, ime.cursor)
    }

    @Test
    fun hangulSyllablesRecomposeInPlace() {
        assertEquals(listOf(Type("ㅎ")), compose("ㅎ"))
        assertEquals(backspaces(1) + Type("하"), compose("하"))
        assertEquals(backspaces(1) + Type("한"), compose("한"))
        assertEquals(emptyList<KeyboardEvent>(), commit("한"))
        assertEquals("한", mac.toString())
    }

    @Test
    fun newlineInsideACompositionIsAReturnKey() {
        compose("ab")
        assertEquals(backspaces(1) + Press(Key.RETURN) + Type("b"), compose("a\nb"))
        finish()
        assertEquals("a\nb", mac.toString())
    }

    @Test
    fun emojiCompositionChangesByWholeCharacters() {
        val thumbs = "👍"
        val medium = "🏽" // skin-tone modifier
        compose("ok $thumbs")
        assertEquals(backspaces(1) + Type(thumbs + medium), compose("ok $thumbs$medium"))
        finish()
        assertEquals("ok $thumbs$medium", mac.toString())
    }

    // ------------------------------------------------------------------ keyboard context

    @Test
    fun lastCommittedCharacterIsTheKeyboardsContext() {
        assertEquals("", ime.textBeforeCursor(10)) // fresh field: start of a sentence
        commit("Hello")
        assertEquals("o", ime.textBeforeCursor(10))
        commit(" ")
        assertEquals(" ", ime.textBeforeCursor(10))
        commit("\n")
        assertEquals("\n", ime.textBeforeCursor(10))
        val thumbsMedium = "\uD83D\uDC4D\uD83C\uDFFD"
        commit(thumbsMedium)
        assertEquals(thumbsMedium, ime.textBeforeCursor(10)) // one whole character
        assertEquals("", ime.textBeforeCursor(1)) // never half a surrogate pair
        commit("\u0007") // dropped control character: nothing sensible to show
        assertEquals("", ime.textBeforeCursor(10))
    }

    @Test
    fun nextDictationSeesTheEndOfThePreviousOne() {
        compose("hello")
        compose("hello world")
        finish()
        // The keyboard sees "d" (not a sentence start) and prepends a space to the next phrase.
        assertEquals("d", ime.textBeforeCursor(1))
        compose(" how")
        commit(" how are you")
        assertEquals("hello world how are you", mac.toString())
        assertEquals("u", ime.textBeforeCursor(5))
    }

    @Test
    fun deletingTheVisibleCharacterIsABackspaceAndRevealsThePreviousOne() {
        commit("hi there")
        assertEquals("e", ime.textBeforeCursor(10))
        assertEquals(backspaces(1), delete(1))
        assertEquals("r", ime.textBeforeCursor(10))
        assertEquals("hi ther", mac.toString())
    }

    @Test
    fun touchingTheStreamForgetsTheContext() {
        commit("hello ")
        assertTrue(ime.forgetContext())
        assertEquals("", ime.textBeforeCursor(10))
        assertFalse("nothing left: no IME restart needed", ime.forgetContext())
        // A delete of what the IME still believes is there stays one Backspace per unit.
        assertEquals(backspaces(1), delete(1))

        compose("wor")
        assertTrue(ime.forgetContext())
        assertEquals("", ime.textBeforeCursor(10))
        // The old composition is not continued (no Backspaces) at the new cursor place.
        assertEquals(listOf(Type("x")), compose("x"))
    }

    // ------------------------------------------------------------------ delete

    @Test
    fun deleteSurroundingTextSendsBackspacesAndForwardDeletes() {
        commit("abc")
        assertEquals(backspaces(1), delete(1))
        assertEquals("ab", mac.toString())
        assertEquals(backspaces(2) + List(3) { Press(Key.FORWARD_DELETE) }, ime.deleteSurroundingText(2, 3))
        assertEquals(emptyList<KeyboardEvent>(), ime.deleteSurroundingText(0, 0))
    }

    @Test
    fun deletingAnEmojiIsOneBackspace() {
        val grin = "😀"
        val thumbsMedium = "👍🏽"
        val family = "👨\u200D👩\u200D👧"
        commit("a$grin$thumbsMedium$family")
        assertEquals(backspaces(1), delete(family.length))
        assertEquals(backspaces(1), delete(thumbsMedium.length))
        assertEquals(backspaces(1), delete(grin.length))
        assertEquals("a", mac.toString())
        assertEquals(1, ime.cursor)
    }

    @Test
    fun deleteInCodePoints() {
        commit("a😀")
        assertEquals(backspaces(1), mac.apply(ime.deleteSurroundingTextInCodePoints(1, 0)))
        assertEquals("a", mac.toString())
    }

    @Test
    fun deletingBeyondKnownTextIsOneBackspacePerUnitAndCapped() {
        assertEquals(backspaces(3), ime.deleteSurroundingText(3, 0))
        assertEquals(List(2) { Press(Key.FORWARD_DELETE) }, ime.deleteSurroundingText(0, 2))
        val capped = ime.deleteSurroundingText(500, 500)
        assertEquals(ImeTextTranslator.MAX_DELETE_PRESSES, capped.count { it == Press(Key.BACKSPACE) })
        assertEquals(ImeTextTranslator.MAX_DELETE_PRESSES, capped.count { it == Press(Key.FORWARD_DELETE) })
        assertEquals(0, ime.cursor)
    }

    @Test
    fun deleteDuringACompositionKeepsTheComposedText() {
        compose("hello")
        assertEquals(backspaces(1), delete(1))
        assertEquals("l", ime.textBeforeCursor(10))
        assertEquals("hell", mac.toString())
    }

    // ------------------------------------------------------------------ re-opened words

    @Test
    fun imeCanReopenTheWordItJustCommitted() {
        // Gboard after a Backspace: it re-opens "hello" as a composition and corrects it.
        commit("hello ")
        delete(1)
        assertEquals(5, ime.cursor)
        assertEquals(emptyList<KeyboardEvent>(), mac.apply(ime.setComposingRegion(0, 5)))
        assertEquals("hello", ime.textBeforeCursor(10))
        assertEquals(backspaces(2) + Type("p"), compose("help"))
        commit("help ")
        assertEquals("help ", mac.toString())
    }

    @Test
    fun composingRegionThatIsNotJustBeforeTheCursorIsIgnored() {
        commit("hello world")
        mac.apply(ime.setComposingRegion(0, 5))
        assertEquals("d", ime.textBeforeCursor(10)) // no composition
        assertEquals(listOf(Type("!")), compose("!"))
        finish()
        // A region that would split a character is ignored too.
        commit("\uD83D\uDE00")
        mac.apply(ime.setComposingRegion(ime.cursor - 1, ime.cursor))
        assertEquals("\uD83D\uDE00", ime.textBeforeCursor(10))
        assertEquals("hello world!\uD83D\uDE00", mac.toString())
    }

    @Test
    fun replaceTextOfTheLastWord() {
        commit("teh")
        assertEquals(backspaces(2) + Type("he"), mac.apply(ime.replaceText(0, 3, "the")))
        assertEquals("the", mac.toString())
        // A region ending at the cursor whose text is no longer known: deleted blindly, then typed.
        commit("ab")
        ime.onKeyPressed(Press(Key.UP))
        assertEquals(backspaces(2) + Type("xy"), ime.replaceText(ime.cursor - 2, ime.cursor, "xy"))
    }

    // ------------------------------------------------------------------ keys and reset

    @Test
    fun keyPressesKeepTheModelHonest() {
        commit("a😀")
        ime.onKeyPressed(Press(Key.BACKSPACE)) // the IME sent DEL as a key event: the emoji is gone
        assertEquals(backspaces(1), ime.deleteSurroundingText(1, 0)) // "a"
        commit("b😀")
        ime.onKeyPressed(Press(Key.LEFT)) // cursor moved on the Mac: nothing before it is known
        assertEquals(backspaces(2), ime.deleteSurroundingText(2, 0))
    }

    @Test
    fun keyPressFinishesTheComposition() {
        compose("abc")
        ime.onKeyPressed(Press(Key.RETURN))
        assertEquals("", ime.textBeforeCursor(10))
        assertEquals(listOf(Type("d")), ime.setComposingText("d"))
    }

    @Test
    fun resetStartsAFreshSession() {
        commit("hello")
        compose("wor")
        ime.reset()
        assertEquals(0, ime.cursor)
        assertEquals("", ime.textBeforeCursor(10))
        assertEquals(listOf(Type("x")), ime.setComposingText("x"))
    }
}
