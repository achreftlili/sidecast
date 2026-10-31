package com.achreftlili.sidecast.ui

import com.achreftlili.sidecast.R
import com.achreftlili.sidecast.net.DiscoveredMac
import com.achreftlili.sidecast.protocol.HostPlatform
import com.achreftlili.sidecast.protocol.ServerAddress
import com.achreftlili.sidecast.store.PairedMac
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MacListTest {
    private fun paired(id: String, name: String, host: String = "10.0.0.9") =
        PairedMac(id, name, "00", "AA==", host, 47800)

    @Test
    fun mergesDiscoveredAndPairedByServerId() {
        val items = MacList.merge(
            discovered = listOf(
                DiscoveredMac("Studio", "id-1", "1", "192.168.1.20", 47800),
                DiscoveredMac("Laptop", null, "1", "192.168.1.30", 47801),
            ),
            paired = listOf(paired("id-1", "Old Studio Name", "192.168.1.5"), paired("id-2", "Office Mac")),
        )
        assertEquals(listOf("Laptop", "Studio", "Office Mac"), items.map { it.name })

        val studio = items.first { it.serverId == "id-1" }
        assertTrue(studio.online)
        assertTrue(studio.paired)
        assertEquals("192.168.1.20", studio.host) // live address beats the stored one

        val laptop = items.first { it.name == "Laptop" }
        assertFalse(laptop.paired)
        assertEquals("nsd:Laptop", laptop.key)
        assertEquals(47801, laptop.port)

        val office = items.last()
        assertFalse(office.online)
        assertTrue(office.paired)
        assertEquals("10.0.0.9", office.host)
    }

    @Test
    fun pairedMacsAwayFromHomeAreRemoteViaTailscale() {
        val withTailscale = paired("id-1", "Studio").copy(addresses = listOf(ServerAddress("100.101.102.103", "tailscale")))
        val lanOnly = paired("id-2", "Office").copy(addresses = listOf(ServerAddress("10.0.0.9", "lan")))
        val noLastHost = paired("id-3", "Laptop", host = "").copy(addresses = listOf(ServerAddress("laptop.tail1.ts.net", "tailscale-dns")))
        val items = MacList.merge(emptyList(), listOf(withTailscale, lanOnly, noLastHost)).associateBy { it.serverId }
        assertTrue(items.getValue("id-1").remote)
        assertFalse(items.getValue("id-2").remote)
        assertTrue(items.getValue("id-3").remote)
        assertEquals("laptop.tail1.ts.net", items.getValue("id-3").host) // still tappable
        // On the local network it is simply online.
        val home = MacList.merge(listOf(DiscoveredMac("Studio", "id-1", "1", "192.168.1.20", 47800)), listOf(withTailscale))
        assertFalse(home.single().remote)
        assertTrue(home.single().online)
    }

    @Test
    fun platformComesFromTxtWhenDiscoveredElseFromThePairing() {
        val items = MacList.merge(
            discovered = listOf(
                DiscoveredMac("Gaming PC", "id-1", "1", "192.168.1.40", 47800, platform = HostPlatform.WINDOWS),
                DiscoveredMac("Old Mac", "id-2", "1", "192.168.1.41", 47800), // no TXT os
                DiscoveredMac("Future", "id-3", "1", "192.168.1.42", 47800, platform = "haiku"),
            ),
            paired = listOf(
                paired("id-1", "Gaming PC").copy(platform = HostPlatform.MACOS), // the live TXT record wins
                paired("id-4", "Workstation").copy(platform = HostPlatform.LINUX),
                paired("id-5", "Office Mac"), // stored before platforms existed
            ),
        ).associateBy { it.serverId }
        assertEquals(HostPlatform.WINDOWS, items.getValue("id-1").platform)
        assertEquals(HostPlatform.MACOS, items.getValue("id-2").platform)
        assertEquals("haiku", items.getValue("id-3").platform)
        assertEquals(HostPlatform.LINUX, items.getValue("id-4").platform)
        assertEquals(HostPlatform.MACOS, items.getValue("id-5").platform)
    }

    @Test
    fun platformLabels() {
        assertEquals(R.string.platform_macos, PlatformLabel.of(HostPlatform.MACOS))
        assertEquals(R.string.platform_windows, PlatformLabel.of(HostPlatform.WINDOWS))
        assertEquals(R.string.platform_linux, PlatformLabel.of(HostPlatform.LINUX))
        // Unknown: no label, a generic computer.
        assertNull(PlatformLabel.of("haiku"))
        assertNull(PlatformLabel.of(""))
        assertNull(PlatformLabel.of(null))
    }

    @Test
    fun duplicateAdvertisementsCollapse() {
        val items = MacList.merge(
            listOf(DiscoveredMac("Studio", "id-1", "1", "192.168.1.20", 47800), DiscoveredMac("Studio (2)", "id-1", "1", "fe80::1", 47800)),
            emptyList(),
        )
        assertEquals(1, items.size)
    }

    @Test
    fun flagsIncompatibleVersions() {
        val items = MacList.merge(listOf(DiscoveredMac("Next", "id-9", "2", "192.168.1.2", 47800)), emptyList())
        assertFalse(items.single().compatible)
        assertTrue(MacList.merge(listOf(DiscoveredMac("NoTxt", null, null, "h", 1)), emptyList()).single().compatible)
    }

    @Test
    fun hostInputParsing() {
        assertEquals(HostInput.Parsed("192.168.1.20", 47800), HostInput.parse(" 192.168.1.20 ", ""))
        assertEquals(HostInput.Parsed("192.168.1.20", 47801), HostInput.parse("192.168.1.20", "47801"))
        assertEquals(HostInput.Parsed("192.168.1.20", 5000), HostInput.parse("192.168.1.20:5000", "47800"))
        assertEquals(HostInput.Parsed("mac.local", 47800), HostInput.parse("mac.local", "47800"))
        assertEquals(HostInput.Parsed("fe80::1%wlan0", 47800), HostInput.parse("fe80::1%wlan0", ""))
        assertEquals(HostInput.Parsed("fe80::1", 6000), HostInput.parse("[fe80::1]:6000", ""))
        assertNull(HostInput.parse("", "47800"))
        assertNull(HostInput.parse("my mac", "47800"))
        assertNull(HostInput.parse("192.168.1.20", "70000"))
        assertNull(HostInput.parse("192.168.1.20", "0"))
        assertNull(HostInput.parse("192.168.1.20", "port"))
        assertNull(HostInput.parse("192.168.1.20:x", ""))
        assertNull(HostInput.parse("[fe80::1", ""))
    }
}
