package com.achreftlili.sidecast.ui

import com.achreftlili.sidecast.net.DiscoveredMac
import com.achreftlili.sidecast.protocol.Protocol
import com.achreftlili.sidecast.store.PairedMac

/** One row of the Mac list: discovered and/or paired, merged by serverId. */
data class MacListItem(
    val key: String,
    val serverId: String?,
    val name: String,
    /** Where to connect: the live Bonjour address, else the last known one. */
    val host: String?,
    val port: Int,
    val online: Boolean,
    val paired: Boolean,
    val protocolVersion: String?,
    /** Not on this network, but paired with a Tailscale address: reachable remotely (§1.1). */
    val remote: Boolean = false,
) {
    val compatible: Boolean get() = protocolVersion == null || protocolVersion == Protocol.VERSION.toString()
}

object MacList {
    /** Online Macs first, then alphabetical. A Mac advertised twice keeps its first entry. */
    fun merge(discovered: List<DiscoveredMac>, paired: List<PairedMac>): List<MacListItem> {
        val pairedById = paired.associateBy { it.serverId }
        val items = LinkedHashMap<String, MacListItem>()
        for (mac in discovered) {
            val key = mac.serverId ?: "nsd:${mac.serviceName}"
            if (key in items) continue
            items[key] = MacListItem(
                key = key,
                serverId = mac.serverId,
                name = mac.serviceName,
                host = mac.host,
                port = mac.port,
                online = true,
                paired = mac.serverId != null && mac.serverId in pairedById,
                protocolVersion = mac.protocolVersion,
            )
        }
        for (mac in paired) {
            if (mac.serverId in items) continue
            items[mac.serverId] = MacListItem(
                key = mac.serverId,
                serverId = mac.serverId,
                name = mac.serverName.ifBlank { mac.lastHost },
                // The connect plan (ConnectPlanner) also tries every stored address from here.
                host = mac.lastHost.ifBlank { null } ?: mac.addresses.firstOrNull()?.host,
                port = mac.lastPort.takeIf { it in 1..65535 } ?: Protocol.DEFAULT_PORT,
                online = false,
                paired = true,
                protocolVersion = null,
                remote = mac.hasTailscale,
            )
        }
        return items.values.sortedWith(compareByDescending<MacListItem> { it.online }.thenBy { it.name.lowercase() })
    }
}
