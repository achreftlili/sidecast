package com.achreftlili.sidecast.net

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.util.Log
import java.io.IOException
import java.net.InetAddress
import java.net.Socket

/**
 * Routes the socket through the network whose subnet contains the Mac — for private LAN
 * destinations only (§1.1).
 *
 * When the Wi-Fi has no internet access Android makes mobile data the default network and an
 * unbound socket to 192.168.x.y silently goes to the carrier. Binding to the on-link network
 * fixes that. When the phone *is* the hotspot no regular network matches and default routing
 * (which knows the tethering subnet) is kept. Tailscale (100.64.0.0/10) and public destinations
 * are never bound: they must follow the default route (the VPN, or mobile data).
 */
class LanSocketBinder(context: Context) {
    private val connectivity = context.applicationContext.getSystemService(ConnectivityManager::class.java)

    fun bind(socket: Socket, target: InetAddress) {
        if (!AddressClass.isPrivateLan(target)) return
        val network = findNetworkFor(target) ?: return
        try {
            network.bindSocket(socket)
        } catch (e: IOException) {
            // e.g. a VPN that forbids bypass; fall back to default routing.
            Log.w(TAG, "could not bind socket to $network: ${e.message}")
        }
    }

    // allNetworks is deprecated in favour of callbacks, but a one-shot lookup is all we need.
    @Suppress("DEPRECATION")
    private fun findNetworkFor(target: InetAddress): Network? = try {
        connectivity.allNetworks.firstOrNull { network ->
            connectivity.getLinkProperties(network)?.linkAddresses?.any {
                Subnet.contains(it.address, it.prefixLength, target)
            } == true
        }
    } catch (e: SecurityException) {
        null
    }

    private companion object {
        const val TAG = "Sidecast/Net"
    }
}
