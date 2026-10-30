package com.achreftlili.sidecast.store

import android.content.SharedPreferences
import com.achreftlili.sidecast.protocol.InputModeName
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** In-memory SharedPreferences (the unit-test android.jar only has throwing stubs). */
private class FakePrefs : SharedPreferences {
    val values = HashMap<String, Any?>()

    override fun getAll(): Map<String, *> = HashMap(values)

    override fun getString(key: String, defValue: String?): String? = values[key] as? String ?: defValue

    override fun getStringSet(key: String, defValues: Set<String>?): Set<String>? =
        @Suppress("UNCHECKED_CAST") (values[key] as? Set<String>) ?: defValues

    override fun getInt(key: String, defValue: Int): Int = values[key] as? Int ?: defValue

    override fun getLong(key: String, defValue: Long): Long = values[key] as? Long ?: defValue

    override fun getFloat(key: String, defValue: Float): Float = values[key] as? Float ?: defValue

    override fun getBoolean(key: String, defValue: Boolean): Boolean = values[key] as? Boolean ?: defValue

    override fun contains(key: String): Boolean = key in values

    override fun edit(): SharedPreferences.Editor = object : SharedPreferences.Editor {
        private val pending = HashMap<String, Any?>()
        private var clear = false

        override fun putString(key: String, value: String?) = apply { pending[key] = value }

        override fun putStringSet(key: String, values: Set<String>?) = apply { pending[key] = values }

        override fun putInt(key: String, value: Int) = apply { pending[key] = value }

        override fun putLong(key: String, value: Long) = apply { pending[key] = value }

        override fun putFloat(key: String, value: Float) = apply { pending[key] = value }

        override fun putBoolean(key: String, value: Boolean) = apply { pending[key] = value }

        override fun remove(key: String) = apply { pending[key] = null }

        override fun clear() = apply { clear = true }

        override fun commit(): Boolean {
            apply()
            return true
        }

        override fun apply() {
            if (clear) values.clear()
            for ((k, v) in pending) if (v == null) values.remove(k) else values[k] = v
        }
    }

    override fun registerOnSharedPreferenceChangeListener(listener: SharedPreferences.OnSharedPreferenceChangeListener) {}

    override fun unregisterOnSharedPreferenceChangeListener(listener: SharedPreferences.OnSharedPreferenceChangeListener) {}
}

class AppSettingsTest {
    private val prefs = FakePrefs()

    @Test
    fun defaults() {
        val settings = AppSettings(prefs)
        assertEquals(InputModeName.TOUCH, settings.inputMode)
        assertTrue(settings.micEnabled)
        assertTrue(settings.dataSaverOnMobile)
    }

    @Test
    fun choicesPersistAcrossInstances() {
        AppSettings(prefs).apply {
            inputMode = InputModeName.TRACKPAD
            micEnabled = false
            dataSaverOnMobile = false
        }
        val reopened = AppSettings(prefs) // e.g. the next app start
        assertEquals(InputModeName.TRACKPAD, reopened.inputMode)
        assertFalse(reopened.micEnabled)
        assertFalse(reopened.dataSaverOnMobile)
        reopened.inputMode = InputModeName.TOUCH
        assertEquals(InputModeName.TOUCH, AppSettings(prefs).inputMode)
    }

    @Test
    fun unknownStoredModeReadsAsTouch() {
        prefs.values["input_mode"] = "pen"
        assertEquals(InputModeName.TOUCH, AppSettings(prefs).inputMode)
    }
}
