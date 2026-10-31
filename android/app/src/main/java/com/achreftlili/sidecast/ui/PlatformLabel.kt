package com.achreftlili.sidecast.ui

import androidx.annotation.StringRes
import com.achreftlili.sidecast.R
import com.achreftlili.sidecast.protocol.HostPlatform

object PlatformLabel {
    /** The name of a host OS ([HostPlatform]); null for one this version does not know: a generic computer. */
    @StringRes
    fun of(platform: String?): Int? = when (platform) {
        HostPlatform.MACOS -> R.string.platform_macos
        HostPlatform.WINDOWS -> R.string.platform_windows
        HostPlatform.LINUX -> R.string.platform_linux
        else -> null
    }
}
