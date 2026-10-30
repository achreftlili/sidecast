package com.achreftlili.sidecast.net

/** A Mac advertising `_sidecast._tcp`, resolved to an address. */
data class DiscoveredMac(
    /** Bonjour instance name: the Mac's computer name. */
    val serviceName: String,
    /** TXT `id`; null if the Mac did not publish one. */
    val serverId: String?,
    /** TXT `v`. */
    val protocolVersion: String?,
    val host: String,
    val port: Int,
)
