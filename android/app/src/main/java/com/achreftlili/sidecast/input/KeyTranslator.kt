package com.achreftlili.sidecast.input

import android.view.KeyCharacterMap
import android.view.KeyEvent
import com.achreftlili.sidecast.protocol.Key

/**
 * Android key events (hardware keyboard, or key events an IME sends) -> KEY / TEXT (§7.1). Pure:
 * takes the KeyEvent fields as ints (only compile-time KeyEvent constants are used), so it runs in
 * JVM unit tests.
 */
object KeyTranslator {
    /** Android key code -> protocol [Key] constant, or null when the protocol has no such key. */
    fun protocolKey(keyCode: Int): Int? = when (keyCode) {
        KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> Key.RETURN
        KeyEvent.KEYCODE_DEL -> Key.BACKSPACE
        KeyEvent.KEYCODE_FORWARD_DEL -> Key.FORWARD_DELETE
        KeyEvent.KEYCODE_TAB -> Key.TAB
        KeyEvent.KEYCODE_ESCAPE -> Key.ESCAPE
        KeyEvent.KEYCODE_DPAD_LEFT -> Key.LEFT
        KeyEvent.KEYCODE_DPAD_RIGHT -> Key.RIGHT
        KeyEvent.KEYCODE_DPAD_UP -> Key.UP
        KeyEvent.KEYCODE_DPAD_DOWN -> Key.DOWN
        KeyEvent.KEYCODE_MOVE_HOME -> Key.HOME
        KeyEvent.KEYCODE_MOVE_END -> Key.END
        KeyEvent.KEYCODE_PAGE_UP -> Key.PAGE_UP
        KeyEvent.KEYCODE_PAGE_DOWN -> Key.PAGE_DOWN
        else -> null
    }

    /** KeyEvent meta state -> `Key.MOD_*` bits (Meta = Command, Alt = Option). */
    fun modifiers(metaState: Int): Int {
        var m = 0
        if (metaState and KeyEvent.META_SHIFT_ON != 0) m = m or Key.MOD_SHIFT
        if (metaState and KeyEvent.META_CTRL_ON != 0) m = m or Key.MOD_CONTROL
        if (metaState and KeyEvent.META_ALT_ON != 0) m = m or Key.MOD_OPTION
        if (metaState and KeyEvent.META_META_ON != 0) m = m or Key.MOD_COMMAND
        return m
    }

    /**
     * A key-down. [unicodeChar] is `KeyEvent.getUnicodeChar(metaState)`. Returns null for keys
     * Sidecast leaves to Android: BACK (it keeps its "disconnect?" meaning), modifier keys alone,
     * dead keys, and Ctrl/Meta shortcuts on printable keys (the protocol has no letter keys).
     */
    fun translate(keyCode: Int, metaState: Int, unicodeChar: Int): KeyboardEvent? {
        if (keyCode == KeyEvent.KEYCODE_BACK) return null
        protocolKey(keyCode)?.let { return KeyboardEvent.Press(it, modifiers(metaState)) }
        if (unicodeChar == 0 || unicodeChar and KeyCharacterMap.COMBINING_ACCENT != 0) return null
        if (metaState and (KeyEvent.META_CTRL_ON or KeyEvent.META_META_ON) != 0) return null
        if (!Character.isValidCodePoint(unicodeChar) || Character.isISOControl(unicodeChar)) return null
        return KeyboardEvent.Type(String(Character.toChars(unicodeChar)))
    }
}
