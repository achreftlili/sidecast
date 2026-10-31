package com.achreftlili.sidecast.store

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import com.achreftlili.sidecast.protocol.InputModeName

/** User preferences (private SharedPreferences; backups are disabled in the manifest). */
class AppSettings(private val prefs: SharedPreferences) {
    constructor(context: Context) : this(context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE))

    /** "Use phone as microphone" (§7.2). When off, the computer's requests are answered `disabled`. */
    var micEnabled: Boolean
        get() = prefs.getBoolean(KEY_MIC_ENABLED, true)
        set(value) = prefs.edit { putBoolean(KEY_MIC_ENABLED, value) }

    /** "Data saver on mobile data" (§5 `limits`): ≤ 4 Mbps and 30 fps on a metered network. */
    var dataSaverOnMobile: Boolean
        get() = prefs.getBoolean(KEY_DATA_SAVER, true)
        set(value) = prefs.edit { putBoolean(KEY_DATA_SAVER, value) }

    /** Touch screen or trackpad (§7.3), an [InputModeName]; anything unknown reads as touch. */
    var inputMode: String
        get() = prefs.getString(KEY_INPUT_MODE, null)?.takeIf { it == InputModeName.TRACKPAD } ?: InputModeName.TOUCH
        set(value) = prefs.edit { putString(KEY_INPUT_MODE, value) }

    private companion object {
        const val FILE = "sidecast_settings"
        const val KEY_MIC_ENABLED = "mic_enabled"
        const val KEY_DATA_SAVER = "data_saver_on_mobile"
        const val KEY_INPUT_MODE = "input_mode"
    }
}
