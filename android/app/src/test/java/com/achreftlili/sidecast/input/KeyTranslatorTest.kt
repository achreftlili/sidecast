package com.achreftlili.sidecast.input

import android.view.KeyCharacterMap
import android.view.KeyEvent
import com.achreftlili.sidecast.input.KeyboardEvent.Press
import com.achreftlili.sidecast.input.KeyboardEvent.Type
import com.achreftlili.sidecast.protocol.Key
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class KeyTranslatorTest {
    @Test
    fun everyMappedKeyCode() {
        val expected = mapOf(
            KeyEvent.KEYCODE_ENTER to Key.RETURN,
            KeyEvent.KEYCODE_NUMPAD_ENTER to Key.RETURN,
            KeyEvent.KEYCODE_DEL to Key.BACKSPACE,
            KeyEvent.KEYCODE_FORWARD_DEL to Key.FORWARD_DELETE,
            KeyEvent.KEYCODE_TAB to Key.TAB,
            KeyEvent.KEYCODE_ESCAPE to Key.ESCAPE,
            KeyEvent.KEYCODE_DPAD_LEFT to Key.LEFT,
            KeyEvent.KEYCODE_DPAD_RIGHT to Key.RIGHT,
            KeyEvent.KEYCODE_DPAD_UP to Key.UP,
            KeyEvent.KEYCODE_DPAD_DOWN to Key.DOWN,
            KeyEvent.KEYCODE_MOVE_HOME to Key.HOME,
            KeyEvent.KEYCODE_MOVE_END to Key.END,
            KeyEvent.KEYCODE_PAGE_UP to Key.PAGE_UP,
            KeyEvent.KEYCODE_PAGE_DOWN to Key.PAGE_DOWN,
        )
        for ((keyCode, key) in expected) {
            assertEquals("keyCode $keyCode", key, KeyTranslator.protocolKey(keyCode))
            assertEquals(Press(key), KeyTranslator.translate(keyCode, 0, 0))
        }
        // Protocol key values (§7.1).
        assertEquals((1..13).toList(), expected.values.distinct().sorted())
    }

    @Test
    fun modifiersFollowTheMetaState() {
        assertEquals(0, KeyTranslator.modifiers(0))
        assertEquals(Key.MOD_SHIFT, KeyTranslator.modifiers(KeyEvent.META_SHIFT_ON or KeyEvent.META_SHIFT_LEFT_ON))
        assertEquals(Key.MOD_CONTROL, KeyTranslator.modifiers(KeyEvent.META_CTRL_ON or KeyEvent.META_CTRL_RIGHT_ON))
        assertEquals(Key.MOD_OPTION, KeyTranslator.modifiers(KeyEvent.META_ALT_ON or KeyEvent.META_ALT_LEFT_ON))
        assertEquals(Key.MOD_COMMAND, KeyTranslator.modifiers(KeyEvent.META_META_ON or KeyEvent.META_META_LEFT_ON))
        assertEquals(
            15,
            KeyTranslator.modifiers(
                KeyEvent.META_SHIFT_ON or KeyEvent.META_CTRL_ON or KeyEvent.META_ALT_ON or KeyEvent.META_META_ON,
            ),
        )
        // Lock states are not modifiers.
        assertEquals(0, KeyTranslator.modifiers(KeyEvent.META_CAPS_LOCK_ON or KeyEvent.META_NUM_LOCK_ON))
    }

    @Test
    fun keysWithModifiers() {
        assertEquals(Press(Key.LEFT, Key.MOD_SHIFT), KeyTranslator.translate(KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.META_SHIFT_ON, 0))
        assertEquals(
            Press(Key.BACKSPACE, Key.MOD_OPTION),
            KeyTranslator.translate(KeyEvent.KEYCODE_DEL, KeyEvent.META_ALT_ON, 0),
        )
        assertEquals(
            Press(Key.TAB, Key.MOD_SHIFT),
            KeyTranslator.translate(KeyEvent.KEYCODE_TAB, KeyEvent.META_SHIFT_ON, '\t'.code),
        )
        // ENTER's unicodeChar is '\n': still the Return key, never TEXT.
        assertEquals(Press(Key.RETURN), KeyTranslator.translate(KeyEvent.KEYCODE_ENTER, 0, '\n'.code))
    }

    @Test
    fun printableKeysAreText() {
        assertEquals(Type("a"), KeyTranslator.translate(KeyEvent.KEYCODE_A, 0, 'a'.code))
        assertEquals(Type("A"), KeyTranslator.translate(KeyEvent.KEYCODE_A, KeyEvent.META_SHIFT_ON, 'A'.code))
        assertEquals(Type(" "), KeyTranslator.translate(KeyEvent.KEYCODE_SPACE, 0, ' '.code))
        assertEquals(Type("é"), KeyTranslator.translate(KeyEvent.KEYCODE_E, KeyEvent.META_ALT_ON, 'é'.code))
        assertEquals(Type("😀"), KeyTranslator.translate(KeyEvent.KEYCODE_UNKNOWN, 0, 0x1F600))
    }

    @Test
    fun keysLeftToAndroid() {
        // BACK keeps its "disconnect?" meaning.
        assertNull(KeyTranslator.translate(KeyEvent.KEYCODE_BACK, 0, 0))
        // Modifier keys alone, keys without a character.
        assertNull(KeyTranslator.translate(KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.META_SHIFT_ON, 0))
        assertNull(KeyTranslator.translate(KeyEvent.KEYCODE_VOLUME_UP, 0, 0))
        // Shortcuts on letters: the protocol has no letter keys.
        assertNull(KeyTranslator.translate(KeyEvent.KEYCODE_C, KeyEvent.META_CTRL_ON, 'c'.code))
        assertNull(KeyTranslator.translate(KeyEvent.KEYCODE_C, KeyEvent.META_META_ON, 'c'.code))
        // Dead keys, control characters, invalid code points.
        assertNull(KeyTranslator.translate(KeyEvent.KEYCODE_GRAVE, 0, KeyCharacterMap.COMBINING_ACCENT or 0x0300))
        assertNull(KeyTranslator.translate(KeyEvent.KEYCODE_UNKNOWN, 0, 0x1B))
        assertNull(KeyTranslator.translate(KeyEvent.KEYCODE_UNKNOWN, 0, 0x110000))
    }
}
