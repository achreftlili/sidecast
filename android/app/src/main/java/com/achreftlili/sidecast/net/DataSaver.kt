package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.Hello
import com.achreftlili.sidecast.protocol.NetworkKind
import com.achreftlili.sidecast.protocol.StreamLimits

/** HELLO `network` / `limits` (§5) for the data saver on mobile data. Pure. */
object DataSaver {
    /** Default data-saver caps (§5): 4 Mbps, 30 fps. */
    val LIMITS = StreamLimits(maxBitrateKbps = 4_000, maxFps = 30)

    /**
     * [hello] for a connection made now: `network` always tells whether the active network is
     * [metered] (true through a VPN on top of mobile data too); `limits` ask for the data-saver
     * caps only on a metered network with the setting [enabled].
     */
    fun apply(hello: Hello, metered: Boolean, enabled: Boolean): Hello = hello.copy(
        network = if (metered) NetworkKind.METERED else NetworkKind.UNMETERED,
        limits = if (metered && enabled) LIMITS else null,
    )
}
