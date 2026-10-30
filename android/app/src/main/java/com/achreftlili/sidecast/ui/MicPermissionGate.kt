package com.achreftlili.sidecast.ui

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Handler
import android.os.Looper
import androidx.core.content.ContextCompat
import com.achreftlili.sidecast.audio.MicPermission

/**
 * RECORD_AUDIO for the [com.achreftlili.sidecast.audio.MicStreamer]. Lives in the ViewModel, so a
 * request survives the activity being recreated; the visible DisplayActivity installs [launcher]
 * (the system permission dialog) and reports back through [onResult].
 */
internal class MicPermissionGate(context: Context) : MicPermission {
    private val app = context.applicationContext
    private val main = Handler(Looper.getMainLooper())

    // Main thread only.
    private var pending: ((Boolean) -> Unit)? = null

    /** Launches the system dialog; set by the visible activity, null otherwise. Main thread. */
    var launcher: (() -> Unit)? = null

    override fun isGranted(): Boolean =
        ContextCompat.checkSelfPermission(app, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    override fun request(onResult: (Boolean) -> Unit): Boolean {
        main.post {
            val launch = launcher
            if (launch == null) {
                onResult(isGranted())
            } else {
                pending = onResult
                launch()
            }
        }
        return true
    }

    /** The dialog's answer (main thread). */
    fun onResult(granted: Boolean) {
        val callback = pending ?: return
        pending = null
        callback(granted)
    }
}
