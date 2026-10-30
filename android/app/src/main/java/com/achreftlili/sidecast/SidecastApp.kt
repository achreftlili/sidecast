package com.achreftlili.sidecast

import android.app.Application
import com.google.android.material.color.DynamicColors

class SidecastApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // Material You colors on Android 12+, the Sidecast palette elsewhere.
        DynamicColors.applyToActivitiesIfAvailable(this)
    }
}
