package com.achreftlili.sidecast.net

import java.net.Inet4Address
import java.net.InetAddress
import java.net.UnknownHostException

/** How the phone reaches the Mac, as shown to the user. */
enum class Route { LAN, TAILSCALE, INTERNET }

/**
 * Classifies destination addresses (§1.1). Pure: only parses numeric literals, never resolves
 * names. Binding a socket to the Wi-Fi network is allowed only for [Kind.PRIVATE_LAN].
 */
object AddressClass {
    enum class Kind { LOOPBACK, PRIVATE_LAN, TAILSCALE, OTHER }

    private class Block(network: String, val prefix: Int) {
        val address: InetAddress = InetAddress.getByName(network)
    }

    private val PRIVATE_V4 = listOf(
        Block("10.0.0.0", 8),
        Block("172.16.0.0", 12),
        Block("192.168.0.0", 16),
        Block("169.254.0.0", 16), // link-local
    )

    /** CGNAT shared space; Tailscale assigns its IPv4 addresses from it. */
    private val TAILSCALE_V4 = Block("100.64.0.0", 10)

    /** Tailscale's IPv6 range; lies inside the ULA block, so it is checked first. */
    private val TAILSCALE_V6 = Block("fd7a:115c:a1e0::", 48)
    private val PRIVATE_V6 = listOf(
        Block("fc00::", 7), // unique local
        Block("fe80::", 10), // link-local
    )

    fun of(address: InetAddress): Kind {
        if (address.isLoopbackAddress) return Kind.LOOPBACK
        fun inAny(blocks: List<Block>) = blocks.any { Subnet.contains(it.address, it.prefix, address) }
        return if (address is Inet4Address) {
            when {
                inAny(PRIVATE_V4) -> Kind.PRIVATE_LAN
                inAny(listOf(TAILSCALE_V4)) -> Kind.TAILSCALE
                else -> Kind.OTHER
            }
        } else {
            when {
                inAny(listOf(TAILSCALE_V6)) -> Kind.TAILSCALE
                inAny(PRIVATE_V6) -> Kind.PRIVATE_LAN
                else -> Kind.OTHER
            }
        }
    }

    /** RFC 1918 / link-local / ULA: the only destinations a socket may be bound to Wi-Fi for. */
    fun isPrivateLan(address: InetAddress): Boolean = of(address) == Kind.PRIVATE_LAN

    fun route(address: InetAddress): Route = when (of(address)) {
        Kind.LOOPBACK, Kind.PRIVATE_LAN -> Route.LAN
        Kind.TAILSCALE -> Route.TAILSCALE
        Kind.OTHER -> Route.INTERNET
    }

    /**
     * [host] parsed as a numeric IPv4 / IPv6 literal (brackets and an IPv6 `%scope` are
     * ignored), or null for a host name. Never does a DNS lookup.
     */
    fun literal(host: String): InetAddress? {
        val h = host.trim().removePrefix("[").removeSuffix("]").substringBefore('%')
        if (':' !in h) {
            val parts = h.split('.')
            if (parts.size != 4) return null
            val bytes = ByteArray(4)
            for ((i, part) in parts.withIndex()) {
                if (part.isEmpty() || part.length > 3 || !part.all { it in '0'..'9' }) return null
                val v = part.toInt()
                if (v > 255) return null
                bytes[i] = v.toByte()
            }
            return InetAddress.getByAddress(bytes)
        }
        if (!h.all { it.isHexDigit() || it == ':' || it == '.' }) return null
        return try {
            InetAddress.getByName("[$h]") // bracketed numeric form: parsed, never looked up
        } catch (_: UnknownHostException) {
            null
        }
    }

    private fun Char.isHexDigit() = this in '0'..'9' || this in 'a'..'f' || this in 'A'..'F'
}
