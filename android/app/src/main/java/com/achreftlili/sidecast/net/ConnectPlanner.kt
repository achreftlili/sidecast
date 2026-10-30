package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.AddressKind
import com.achreftlili.sidecast.store.PairedMac
import java.io.IOException

/** One address to try for a Mac, with the route it implies and its TCP connect timeout. */
data class ConnectCandidate(
    val host: String,
    val port: Int,
    val route: Route,
    val connectTimeoutMs: Int,
)

/** Which addresses to try, in order, to reach a Mac (§1.1). Pure. */
object ConnectPlanner {
    const val LAN_CONNECT_TIMEOUT_MS = 2_500
    const val REMOTE_CONNECT_TIMEOUT_MS = 6_000

    private val STORED_KIND_ORDER = listOf(AddressKind.LAN, AddressKind.TAILSCALE, AddressKind.TAILSCALE_DNS)

    /**
     * @param host the address the user picked (list row or "Add by IP address")
     * @param port its port
     * @param discoveredHost the Mac's Bonjour-resolved address right now, or null
     * @param paired the stored pairing; null when not paired (or not known yet)
     * @return Bonjour address only when discovered; the picked address alone for an unpaired Mac;
     *   otherwise lastHost, the picked address, then the stored lan, tailscale and tailscale-dns
     *   addresses — duplicates removed, unknown kinds ignored
     */
    fun plan(host: String, port: Int, discoveredHost: String?, paired: PairedMac?): List<ConnectCandidate> {
        if (discoveredHost != null) return listOf(candidate(discoveredHost, port, AddressKind.LAN))
        if (paired == null) return listOf(candidate(host, port, kind = null))
        val lastPort = paired.lastPort.takeIf(::isPort) ?: port
        val addressPort = paired.serverPort.takeIf(::isPort) ?: lastPort
        val ordered = buildList {
            if (paired.lastHost.isNotBlank()) add(candidate(paired.lastHost, lastPort, kind = null))
            if (host.isNotBlank()) add(candidate(host, port, kind = null))
            for (kind in STORED_KIND_ORDER) {
                for (address in paired.addresses) if (address.kind == kind) add(candidate(address.host, addressPort, kind))
            }
        }
        return ordered.distinctBy { it.host.lowercase() to it.port }
    }

    /** Route of [host]: numeric addresses by class, names by their advertised kind or suffix. */
    fun route(host: String, kind: String?): Route {
        AddressClass.literal(host)?.let { return AddressClass.route(it) }
        return when {
            kind == AddressKind.LAN -> Route.LAN
            kind == AddressKind.TAILSCALE || kind == AddressKind.TAILSCALE_DNS -> Route.TAILSCALE
            host.endsWith(".local", ignoreCase = true) || host.endsWith(".local.", ignoreCase = true) -> Route.LAN
            host.endsWith(".ts.net", ignoreCase = true) || host.endsWith(".ts.net.", ignoreCase = true) -> Route.TAILSCALE
            else -> Route.INTERNET
        }
    }

    private fun candidate(host: String, port: Int, kind: String?): ConnectCandidate {
        val route = route(host.trim(), kind)
        val timeout = if (route == Route.LAN) LAN_CONNECT_TIMEOUT_MS else REMOTE_CONNECT_TIMEOUT_MS
        return ConnectCandidate(host.trim(), port, route, timeout)
    }

    private fun isPort(p: Int) = p in 1..65535
}

/**
 * Tries [candidates] in order with a fresh [Connector] each ([connectorFor]); the first that
 * connects wins. Each connector does its own certificate pinning, so the token is never sent to
 * an unpinned server whichever address answers. A pin mismatch at one address does not stop the
 * others, but is what gets reported if none connects. [onAttempt] runs before each try.
 */
class FallbackConnector(
    private val candidates: List<ConnectCandidate>,
    private val connectorFor: (ConnectCandidate) -> Connector,
    private val onAttempt: (ConnectCandidate) -> Unit = {},
) : Connector {
    @Volatile
    private var current: Connector? = null

    @Volatile
    private var cancelled = false

    override fun connect(): Connection {
        var identityChanged: IdentityChangedException? = null
        var lastError: IOException? = null
        for (candidate in candidates) {
            if (cancelled) throw IOException("cancelled")
            onAttempt(candidate)
            val connector = connectorFor(candidate)
            current = connector
            if (cancelled) connector.cancel() // close() raced with the hand-over
            try {
                return connector.connect()
            } catch (e: IdentityChangedException) {
                identityChanged = e
            } catch (e: IOException) {
                lastError = e
            } finally {
                current = null
            }
        }
        if (cancelled) throw IOException("cancelled")
        throw identityChanged ?: lastError ?: IOException("no address to connect to")
    }

    override fun cancel() {
        cancelled = true
        current?.cancel()
    }
}
