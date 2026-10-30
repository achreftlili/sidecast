package com.achreftlili.sidecast.net

import java.net.InetAddress

object Subnet {
    /** True when [target] lies in [network]/[prefixLength] (same address family only). */
    fun contains(network: InetAddress, prefixLength: Int, target: InetAddress): Boolean {
        val a = network.address
        val b = target.address
        if (a.size != b.size || prefixLength < 0 || prefixLength > a.size * 8) return false
        val fullBytes = prefixLength / 8
        for (i in 0 until fullBytes) if (a[i] != b[i]) return false
        val rest = prefixLength % 8
        if (rest == 0) return true
        val mask = (0xFF shl (8 - rest)) and 0xFF
        return (a[fullBytes].toInt() and mask) == (b[fullBytes].toInt() and mask)
    }
}
