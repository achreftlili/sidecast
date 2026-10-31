package com.achreftlili.sidecast.store

import com.achreftlili.sidecast.net.InMemoryPairings
import com.achreftlili.sidecast.protocol.HostPlatform
import com.achreftlili.sidecast.protocol.ServerAddress
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairingStoreFormatTest {
    private val mac = PairedMac(
        serverId = "id-1",
        serverName = "Studio",
        fingerprintHex = "ab".repeat(32),
        tokenB64 = "AAECAw==",
        lastHost = "192.168.1.20",
        lastPort = 47800,
        addresses = listOf(ServerAddress("192.168.1.20", "lan"), ServerAddress("100.101.102.103", "tailscale")),
        serverPort = 47800,
        platform = HostPlatform.WINDOWS,
    )

    @Test
    fun roundTrips() {
        assertEquals(mac, PairingStore.parsePairedMac(PairingStore.encodePairedMac(mac)))
        assertTrue(mac.hasTailscale)
        val unknownOs = mac.copy(platform = "plan9")
        assertEquals(unknownOs, PairingStore.parsePairedMac(PairingStore.encodePairedMac(unknownOs)))
    }

    @Test
    fun pairingsStoredBefore04HaveNoAddresses() {
        // Exactly what 0.1–0.3 wrote.
        val old = """{"serverId":"id-1","serverName":"Studio","fingerprint":"${"ab".repeat(32)}","token":"AAECAw==","lastHost":"192.168.1.20","lastPort":47800}"""
        val parsed = PairingStore.parsePairedMac(old)!!
        assertEquals(mac.copy(addresses = emptyList(), serverPort = 0, platform = HostPlatform.MACOS), parsed)
        assertFalse(parsed.hasTailscale)
    }

    @Test
    fun pairingsStoredBeforeHostsAnnouncedTheirOsAreMacs() {
        // Exactly what 0.4–0.6 wrote.
        val old = """{"serverId":"id-1","serverName":"Studio","fingerprint":"${"ab".repeat(32)}","token":"AAECAw==",""" +
            """"lastHost":"192.168.1.20","lastPort":47800,"serverPort":47800,""" +
            """"addresses":[{"host":"192.168.1.20","kind":"lan"},{"host":"100.101.102.103","kind":"tailscale"}]}"""
        assertEquals(mac.copy(platform = HostPlatform.MACOS), PairingStore.parsePairedMac(old))
    }

    @Test
    fun brokenAddressEntriesAreSkipped() {
        val text = PairingStore.encodePairedMac(mac).replace(
            "\"addresses\":[",
            "\"addresses\":[\"junk\",{\"host\":\"\",\"kind\":\"lan\"},",
        )
        assertEquals(mac.addresses, PairingStore.parsePairedMac(text)!!.addresses)
    }

    @Test
    fun updateEndpointKeepsAddressesUnlessTheMacSentSome() {
        val repo = InMemoryPairings()
        repo.save(mac)
        repo.updateEndpoint("id-1", "Studio", "100.101.102.103", 47800) // older Mac: no addresses
        assertEquals(mac.addresses, repo.get("id-1")!!.addresses)
        assertEquals("100.101.102.103", repo.get("id-1")!!.lastHost)

        val fresh = listOf(ServerAddress("10.0.0.7", "lan"))
        repo.updateEndpoint("id-1", "", "10.0.0.7", 47801, addresses = fresh, serverPort = 47801)
        val updated = repo.get("id-1")!!
        assertEquals(fresh, updated.addresses)
        assertEquals(47801, updated.serverPort)
        assertEquals("Studio", updated.serverName) // a blank name never overwrites
        assertFalse(updated.hasTailscale)
    }

    @Test
    fun updateEndpointKeepsThePlatformUnlessTheHostSentOne() {
        val repo = InMemoryPairings()
        repo.save(mac.copy(platform = HostPlatform.MACOS))
        repo.updateEndpoint("id-1", "Studio", "192.168.1.20", 47800)
        assertEquals(HostPlatform.MACOS, repo.get("id-1")!!.platform)
        repo.updateEndpoint("id-1", "Studio", "192.168.1.20", 47800, platform = HostPlatform.LINUX)
        assertEquals(HostPlatform.LINUX, repo.get("id-1")!!.platform)
    }
}
