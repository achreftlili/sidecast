package com.achreftlili.sidecast.store

import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.core.content.edit
import java.util.UUID

/** Stable per-install identity sent in HELLO / AUTH. */
object DeviceIdentity {
    private const val FILE = "sidecast_device"
    private const val KEY_DEVICE_ID = "device_id"

    /** Random UUID generated on first use and kept for the life of the install. */
    @Synchronized
    fun deviceId(context: Context): String {
        val prefs = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        prefs.getString(KEY_DEVICE_ID, null)?.let { return it }
        val id = UUID.randomUUID().toString()
        prefs.edit { putString(KEY_DEVICE_ID, id) }
        return id
    }

    /** User-visible name: Settings.Global.DEVICE_NAME, falling back to the model. */
    fun deviceName(context: Context): String {
        val name = try {
            Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
        } catch (_: SecurityException) {
            null
        }
        return name?.takeIf { it.isNotBlank() } ?: Build.MODEL
    }
}
