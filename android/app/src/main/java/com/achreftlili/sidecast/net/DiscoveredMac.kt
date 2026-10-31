package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.HostPlatform

/** A computer advertising `_sidecast._tcp`, resolved to an address. */
data class DiscoveredMac(
    /** Bonjour instance name: the computer's name. */
    val serviceName: String,
    /** TXT `id`; null if the host did not publish one. */
    val serverId: String?,
    /** TXT `v`. */
    val protocolVersion: String?,
    val host: String,
    val port: Int,
    /** TXT `os`, a [HostPlatform]; Mac hosts older than it publish none, so absent means macOS. */
    val platform: String = HostPlatform.MACOS,
)
