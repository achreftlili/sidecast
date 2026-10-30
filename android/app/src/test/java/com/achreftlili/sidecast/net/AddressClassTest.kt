package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.net.AddressClass.Kind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AddressClassTest {
    private fun kind(host: String): Kind = AddressClass.of(checkNotNull(AddressClass.literal(host)) { host })

    @Test
    fun rfc1918AndLinkLocalArePrivate() {
        for (h in listOf("10.0.0.0", "10.255.255.255", "192.168.0.1", "192.168.255.255", "169.254.0.1", "169.254.255.254")) {
            assertEquals(h, Kind.PRIVATE_LAN, kind(h))
        }
        assertEquals(Kind.OTHER, kind("192.169.0.1"))
        assertEquals(Kind.OTHER, kind("11.0.0.1"))
        assertEquals(Kind.OTHER, kind("169.253.255.255"))
    }

    @Test
    fun boundariesOf172_16_12() {
        assertEquals(Kind.OTHER, kind("172.15.255.255"))
        assertEquals(Kind.PRIVATE_LAN, kind("172.16.0.0"))
        assertEquals(Kind.PRIVATE_LAN, kind("172.31.255.255"))
        assertEquals(Kind.OTHER, kind("172.32.0.0"))
    }

    @Test
    fun boundariesOfTailscale100_64_10() {
        assertEquals(Kind.OTHER, kind("100.63.255.255"))
        assertEquals(Kind.TAILSCALE, kind("100.64.0.0"))
        assertEquals(Kind.TAILSCALE, kind("100.101.102.103"))
        assertEquals(Kind.TAILSCALE, kind("100.127.255.255"))
        assertEquals(Kind.OTHER, kind("100.128.0.0"))
    }

    @Test
    fun ipv6() {
        assertEquals(Kind.TAILSCALE, kind("fd7a:115c:a1e0::1")) // inside ULA, but Tailscale's
        assertEquals(Kind.TAILSCALE, kind("fd7a:115c:a1e0:ab12:4843:cd96:6258:b240"))
        assertEquals(Kind.PRIVATE_LAN, kind("fd7a:115c:a1e1::1")) // just outside the /48
        assertEquals(Kind.PRIVATE_LAN, kind("fd00::1"))
        assertEquals(Kind.PRIVATE_LAN, kind("fc00::1"))
        assertEquals(Kind.PRIVATE_LAN, kind("fe80::1"))
        assertEquals(Kind.PRIVATE_LAN, kind("fe80::1%wlan0"))
        assertEquals(Kind.PRIVATE_LAN, kind("[febf::1]"))
        assertEquals(Kind.OTHER, kind("fec0::1"))
        assertEquals(Kind.OTHER, kind("2001:db8::1"))
        assertEquals(Kind.LOOPBACK, kind("::1"))
        assertEquals(Kind.LOOPBACK, kind("127.0.0.1"))
    }

    @Test
    fun onlyPrivateLanMayBeBoundToWifi() {
        assertTrue(AddressClass.isPrivateLan(AddressClass.literal("192.168.1.20")!!))
        assertTrue(AddressClass.isPrivateLan(AddressClass.literal("fe80::1")!!))
        assertFalse(AddressClass.isPrivateLan(AddressClass.literal("100.101.102.103")!!)) // VPN route
        assertFalse(AddressClass.isPrivateLan(AddressClass.literal("fd7a:115c:a1e0::1")!!))
        assertFalse(AddressClass.isPrivateLan(AddressClass.literal("8.8.8.8")!!)) // mobile data / Wi-Fi default
        assertFalse(AddressClass.isPrivateLan(AddressClass.literal("127.0.0.1")!!))
    }

    @Test
    fun routes() {
        assertEquals(Route.LAN, AddressClass.route(AddressClass.literal("192.168.1.20")!!))
        assertEquals(Route.LAN, AddressClass.route(AddressClass.literal("127.0.0.1")!!))
        assertEquals(Route.TAILSCALE, AddressClass.route(AddressClass.literal("100.64.0.1")!!))
        assertEquals(Route.INTERNET, AddressClass.route(AddressClass.literal("203.0.113.7")!!))
    }

    @Test
    fun literalsAreParsedNeverResolved() {
        assertNotNull(AddressClass.literal(" 192.168.1.2 "))
        assertNotNull(AddressClass.literal("[2001:db8::1]"))
        for (name in listOf("studio.local", "mac.tail1234.ts.net", "studio", "999.1.1.1", "1.2.3", "1.2.3.4.5", "1..2.3", "١.1.1.1", "abc::g", "")) {
            assertNull(name, AddressClass.literal(name))
        }
    }
}
