package com.achreftlili.sidecast.store

import com.achreftlili.sidecast.protocol.AddressKind
import com.achreftlili.sidecast.protocol.Base64Codec
import com.achreftlili.sidecast.protocol.ServerAddress
import com.achreftlili.sidecast.protocol.hexToBytes

/**
 * What the phone remembers about a paired Mac (§5 "Pairing", step 4). Binary values are kept in
 * their persisted text form (hex fingerprint, Base64 token) so the class has value equality.
 */
data class PairedMac(
    val serverId: String,
    val serverName: String,
    val fingerprintHex: String,
    val tokenB64: String,
    val lastHost: String,
    val lastPort: Int,
    /** Where the Mac says it can be reached (SERVER_HELLO, §1.1); empty for older pairings. */
    val addresses: List<ServerAddress> = emptyList(),
    /** The Mac's listening port from SERVER_HELLO; 0 when it never said. */
    val serverPort: Int = 0,
) {
    val fingerprint: ByteArray get() = hexToBytes(fingerprintHex)
    val token: ByteArray get() = Base64Codec.decode(tokenB64)

    /** Reachable over Tailscale when away from home. */
    val hasTailscale: Boolean
        get() = addresses.any { it.kind == AddressKind.TAILSCALE || it.kind == AddressKind.TAILSCALE_DNS }

    override fun toString(): String =
        "PairedMac(serverId=$serverId, serverName=$serverName, fingerprint=$fingerprintHex, " +
            "lastHost=$lastHost, lastPort=$lastPort, addresses=$addresses, serverPort=$serverPort)"
}

/** Persistence of pairings, keyed by serverId. Implementations must be thread-safe. */
interface PairingRepository {
    fun get(serverId: String): PairedMac?

    fun all(): List<PairedMac>

    fun save(mac: PairedMac)

    fun remove(serverId: String)

    /**
     * Remember where this Mac was last reached and, when SERVER_HELLO said, its addresses and
     * port (null = keep what is stored). No-op when it is not paired.
     */
    fun updateEndpoint(
        serverId: String,
        serverName: String,
        host: String,
        port: Int,
        addresses: List<ServerAddress>? = null,
        serverPort: Int? = null,
    ) {
        val current = get(serverId) ?: return
        val updated = current.copy(
            serverName = serverName.ifBlank { current.serverName },
            lastHost = host,
            lastPort = port,
            addresses = addresses ?: current.addresses,
            serverPort = serverPort ?: current.serverPort,
        )
        if (updated != current) save(updated)
    }
}
