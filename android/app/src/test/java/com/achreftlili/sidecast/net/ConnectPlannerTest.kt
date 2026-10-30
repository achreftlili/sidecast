package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.net.ConnectPlanner.LAN_CONNECT_TIMEOUT_MS
import com.achreftlili.sidecast.net.ConnectPlanner.REMOTE_CONNECT_TIMEOUT_MS
import com.achreftlili.sidecast.protocol.ServerAddress
import com.achreftlili.sidecast.store.PairedMac
import org.junit.Assert.assertEquals
import org.junit.Test

class ConnectPlannerTest {
    private val paired = PairedMac(
        serverId = "id",
        serverName = "Studio",
        fingerprintHex = "00",
        tokenB64 = "AA==",
        lastHost = "192.168.1.20",
        lastPort = 47800,
        addresses = listOf(
            ServerAddress("mac.tail1234.ts.net", "tailscale-dns"),
            ServerAddress("100.101.102.103", "tailscale"),
            ServerAddress("192.168.1.20", "lan"), // same as lastHost
            ServerAddress("10.0.0.5", "lan"),
            ServerAddress("fd7a:115c:a1e0::5", "tailscale"),
            ServerAddress("203.0.113.9", "public"), // unknown kind: ignored
        ),
        serverPort = 47801,
    )

    private fun lan(host: String, port: Int) = ConnectCandidate(host, port, Route.LAN, LAN_CONNECT_TIMEOUT_MS)

    private fun tailscale(host: String, port: Int) = ConnectCandidate(host, port, Route.TAILSCALE, REMOTE_CONNECT_TIMEOUT_MS)

    @Test
    fun bonjourAddressIsTheOnlyCandidateWhenDiscovered() {
        assertEquals(
            listOf(lan("192.168.1.42", 47800)),
            ConnectPlanner.plan("192.168.1.42", 47800, discoveredHost = "192.168.1.42", paired = paired),
        )
    }

    @Test
    fun unpairedMacIsTriedAtThePickedAddressOnly() {
        assertEquals(listOf(lan("192.168.1.9", 47800)), ConnectPlanner.plan("192.168.1.9", 47800, null, null))
        assertEquals(listOf(tailscale("100.70.0.1", 47800)), ConnectPlanner.plan("100.70.0.1", 47800, null, null))
        assertEquals(
            listOf(ConnectCandidate("mac.example.com", 5000, Route.INTERNET, REMOTE_CONNECT_TIMEOUT_MS)),
            ConnectPlanner.plan("mac.example.com", 5000, null, null),
        )
    }

    @Test
    fun pairedMacTriesLastHostThenLanThenTailscaleThenTailscaleDns() {
        val plan = ConnectPlanner.plan("192.168.1.20", 47800, discoveredHost = null, paired = paired)
        assertEquals(
            listOf(
                lan("192.168.1.20", 47800), // lastHost at lastPort
                lan("192.168.1.20", 47801), // advertised lan address at the advertised port
                lan("10.0.0.5", 47801),
                tailscale("100.101.102.103", 47801),
                tailscale("fd7a:115c:a1e0::5", 47801),
                tailscale("mac.tail1234.ts.net", 47801),
            ),
            plan,
        )
    }

    @Test
    fun duplicatesAreRemovedCaseInsensitively() {
        val mac = paired.copy(
            lastHost = "Mac.Tail1234.ts.net",
            serverPort = 0, // older Mac: addresses use lastPort
            addresses = listOf(ServerAddress("mac.tail1234.ts.net", "tailscale-dns"), ServerAddress("100.101.102.103", "tailscale")),
        )
        assertEquals(
            listOf(tailscale("Mac.Tail1234.ts.net", 47800), tailscale("100.101.102.103", 47800)),
            ConnectPlanner.plan("Mac.Tail1234.ts.net", 47800, null, mac),
        )
    }

    @Test
    fun pairingWithoutAddressesStillUsesLastHostAndThePickedHost() {
        val old = paired.copy(addresses = emptyList(), serverPort = 0, lastHost = "")
        assertEquals(listOf(lan("192.168.1.30", 47800)), ConnectPlanner.plan("192.168.1.30", 47800, null, old))
        val moved = paired.copy(addresses = emptyList(), serverPort = 0)
        assertEquals(
            listOf(lan("192.168.1.20", 47800), lan("192.168.1.31", 47800)),
            ConnectPlanner.plan("192.168.1.31", 47800, null, moved),
        )
    }

    @Test
    fun routesAndTimeoutsOfNames() {
        assertEquals(Route.LAN, ConnectPlanner.route("studio.local", null))
        assertEquals(Route.LAN, ConnectPlanner.route("studio.local.", null))
        assertEquals(Route.TAILSCALE, ConnectPlanner.route("studio.tail1234.ts.net", null))
        assertEquals(Route.LAN, ConnectPlanner.route("studio.lan", "lan"))
        assertEquals(Route.TAILSCALE, ConnectPlanner.route("studio", "tailscale-dns"))
        assertEquals(Route.INTERNET, ConnectPlanner.route("mac.example.com", null))
        // A numeric address is classified by itself, whatever the advertised kind says.
        assertEquals(Route.INTERNET, ConnectPlanner.route("203.0.113.9", "lan"))
        assertEquals(
            LAN_CONNECT_TIMEOUT_MS,
            ConnectPlanner.plan("studio.local", 47800, null, null).single().connectTimeoutMs,
        )
    }
}
