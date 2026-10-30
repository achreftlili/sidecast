package com.achreftlili.sidecast.net

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.annotation.RequiresApi
import com.achreftlili.sidecast.protocol.Protocol
import java.net.Inet4Address
import java.net.InetAddress

/**
 * Bonjour discovery via [NsdManager] (§1). All callbacks to [onChange] happen on the main thread.
 *
 * Resolution uses `registerServiceInfoCallback` on API 34+ (live updates, several at once) and
 * the legacy `resolveService` below, which allows only one resolve in flight: requests are
 * queued and FAILURE_ALREADY_ACTIVE is retried.
 */
class MacDiscovery(
    context: Context,
    private val onChange: (List<DiscoveredMac>) -> Unit,
) {
    private val nsd = context.applicationContext.getSystemService(NsdManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private val resolved = LinkedHashMap<String, DiscoveredMac>()
    private val resolver: Resolver =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) CallbackResolver(context) else LegacyResolver()
    private var discoveryListener: NsdManager.DiscoveryListener? = null

    /** Main thread. Idempotent. */
    fun start() {
        if (discoveryListener != null) return
        val listener = DiscoveryListener()
        discoveryListener = listener
        try {
            nsd.discoverServices(Protocol.SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (e: RuntimeException) {
            Log.w(TAG, "discoverServices failed", e)
            discoveryListener = null
        }
    }

    /** Main thread. Idempotent. */
    fun stop() {
        val listener = discoveryListener ?: return
        discoveryListener = null
        try {
            nsd.stopServiceDiscovery(listener)
        } catch (e: RuntimeException) {
            Log.w(TAG, "stopServiceDiscovery failed", e)
        }
        resolver.cancelAll()
        resolved.clear()
        publish()
    }

    private fun publish() = onChange(resolved.values.toList())

    private fun deliver(info: NsdServiceInfo) {
        val address = pickAddress(info) ?: return
        val attributes = info.attributes
        fun txt(key: String) = attributes[key]?.let { String(it, Charsets.UTF_8) }?.trim()?.takeIf { it.isNotEmpty() }
        val host = address.hostAddress ?: return
        resolved[info.serviceName] = DiscoveredMac(
            serviceName = info.serviceName,
            serverId = txt(Protocol.TXT_ID),
            protocolVersion = txt(Protocol.TXT_VERSION),
            host = host,
            port = info.port,
        )
        publish()
    }

    private fun lose(serviceName: String) {
        resolver.cancel(serviceName)
        if (resolved.remove(serviceName) != null) publish()
    }

    private fun pickAddress(info: NsdServiceInfo): InetAddress? {
        val addresses: List<InetAddress> = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            info.hostAddresses
        } else {
            @Suppress("DEPRECATION")
            listOfNotNull(info.host)
        }
        // IPv4 first: IPv6 link-local addresses need a scope and are less reliable.
        return addresses.firstOrNull { it is Inet4Address } ?: addresses.firstOrNull()
    }

    private inner class DiscoveryListener : NsdManager.DiscoveryListener {
        private val isCurrent get() = discoveryListener === this

        override fun onDiscoveryStarted(serviceType: String) {
            Log.d(TAG, "discovery started")
        }

        override fun onDiscoveryStopped(serviceType: String) {
            Log.d(TAG, "discovery stopped")
        }

        override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
            Log.w(TAG, "start discovery failed: $errorCode")
            main.post { if (isCurrent) discoveryListener = null }
        }

        override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
            Log.w(TAG, "stop discovery failed: $errorCode")
        }

        override fun onServiceFound(serviceInfo: NsdServiceInfo) {
            main.post { if (isCurrent) resolver.resolve(serviceInfo) }
        }

        override fun onServiceLost(serviceInfo: NsdServiceInfo) {
            main.post { if (isCurrent) lose(serviceInfo.serviceName) }
        }
    }

    private interface Resolver {
        fun resolve(info: NsdServiceInfo)

        fun cancel(serviceName: String)

        fun cancelAll()
    }

    /** API < 34: one resolve at a time. */
    private inner class LegacyResolver : Resolver {
        private val queue = ArrayDeque<NsdServiceInfo>()
        private var active: String? = null
        private var generation = 0

        override fun resolve(info: NsdServiceInfo) {
            if (active == info.serviceName || queue.any { it.serviceName == info.serviceName }) return
            queue.addLast(info)
            pump()
        }

        override fun cancel(serviceName: String) {
            queue.removeAll { it.serviceName == serviceName }
        }

        override fun cancelAll() {
            queue.clear()
            generation++
        }

        private fun pump() {
            if (active != null) return
            val next = queue.removeFirstOrNull() ?: return
            active = next.serviceName
            val gen = generation
            @Suppress("DEPRECATION")
            nsd.resolveService(
                next,
                object : NsdManager.ResolveListener {
                    override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                        main.post {
                            active = null
                            if (gen == generation && discoveryListener != null) deliver(serviceInfo)
                            pump()
                        }
                    }

                    override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                        main.post {
                            active = null
                            if (errorCode == NsdManager.FAILURE_ALREADY_ACTIVE && gen == generation) {
                                main.postDelayed({ if (gen == generation) resolve(next) }, RETRY_DELAY_MS)
                            } else {
                                Log.w(TAG, "resolve ${serviceInfo.serviceName} failed: $errorCode")
                            }
                            pump()
                        }
                    }
                },
            )
        }
    }

    /** API 34+: live service-info callbacks, no concurrency limit. */
    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private inner class CallbackResolver(context: Context) : Resolver {
        private val executor = context.mainExecutor
        private val callbacks = HashMap<String, NsdManager.ServiceInfoCallback>()

        override fun resolve(info: NsdServiceInfo) {
            val name = info.serviceName
            if (callbacks.containsKey(name)) return
            val callback = object : NsdManager.ServiceInfoCallback {
                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                    Log.w(TAG, "service info callback for $name failed: $errorCode")
                    if (callbacks[name] === this) callbacks.remove(name)
                }

                override fun onServiceUpdated(serviceInfo: NsdServiceInfo) {
                    if (callbacks[name] === this) deliver(serviceInfo)
                }

                override fun onServiceLost() {
                    if (callbacks[name] === this) lose(name)
                }

                override fun onServiceInfoCallbackUnregistered() = Unit
            }
            callbacks[name] = callback
            try {
                nsd.registerServiceInfoCallback(info, executor, callback)
            } catch (e: RuntimeException) {
                Log.w(TAG, "registerServiceInfoCallback failed", e)
                callbacks.remove(name)
            }
        }

        override fun cancel(serviceName: String) {
            callbacks.remove(serviceName)?.let(::unregister)
        }

        override fun cancelAll() {
            callbacks.values.forEach(::unregister)
            callbacks.clear()
        }

        private fun unregister(callback: NsdManager.ServiceInfoCallback) {
            try {
                nsd.unregisterServiceInfoCallback(callback)
            } catch (_: IllegalArgumentException) {
            }
        }
    }

    private companion object {
        const val TAG = "Sidecast/NSD"
        const val RETRY_DELAY_MS = 300L
    }
}
