package com.achreftlili.sidecast.ui

import com.achreftlili.sidecast.protocol.Protocol

/** Parses the "Add by IP address" fields. Pure. */
object HostInput {
    data class Parsed(val host: String, val port: Int)

    /**
     * Accepts `192.168.1.20`, `mac.local`, `192.168.1.20:47801`, `[fe80::1%wlan0]:47800` or a bare
     * IPv6 literal. A port inside [hostText] wins over [portText]; a blank port means the default.
     * Returns null for an empty/invalid host or an out-of-range port.
     */
    fun parse(hostText: String, portText: String): Parsed? {
        var host = hostText.trim()
        var port: Int? = portText.trim().takeIf { it.isNotEmpty() }?.let { it.toIntOrNull() ?: return null }
        if (host.startsWith("[")) {
            val close = host.indexOf(']')
            if (close < 0) return null
            val rest = host.substring(close + 1)
            if (rest.startsWith(":")) port = rest.substring(1).toIntOrNull() ?: return null else if (rest.isNotEmpty()) return null
            host = host.substring(1, close)
        } else if (host.count { it == ':' } == 1) {
            val idx = host.indexOf(':')
            port = host.substring(idx + 1).toIntOrNull() ?: return null
            host = host.substring(0, idx)
        }
        if (host.isEmpty() || host.any { it.isWhitespace() || it == '/' }) return null
        val p = port ?: Protocol.DEFAULT_PORT
        if (p !in 1..65535) return null
        return Parsed(host, p)
    }
}
