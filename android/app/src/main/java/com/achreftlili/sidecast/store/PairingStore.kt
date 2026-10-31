package com.achreftlili.sidecast.store

import android.content.Context
import androidx.core.content.edit
import com.achreftlili.sidecast.protocol.HostPlatform
import com.achreftlili.sidecast.protocol.ServerAddress
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/**
 * [PairingRepository] backed by private SharedPreferences: one JSON entry per serverId.
 * Backups and device-to-device transfer are disabled in the manifest, so tokens never leave
 * the phone.
 */
class PairingStore(context: Context) : PairingRepository {
    private val prefs = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    override fun get(serverId: String): PairedMac? =
        prefs.getString(KEY_PREFIX + serverId, null)?.let(::parse)

    override fun all(): List<PairedMac> =
        prefs.all.entries
            .filter { it.key.startsWith(KEY_PREFIX) }
            .mapNotNull { (it.value as? String)?.let(::parse) }
            .sortedBy { it.serverName.lowercase() }

    // commit: a freshly issued token must survive the process dying right after pairing.
    // Callers are network threads, never the main thread.
    override fun save(mac: PairedMac) {
        prefs.edit(commit = true) { putString(KEY_PREFIX + mac.serverId, encodePairedMac(mac)) }
    }

    override fun remove(serverId: String) {
        prefs.edit { remove(KEY_PREFIX + serverId) }
    }

    private fun parse(text: String): PairedMac? = parsePairedMac(text)

    companion object {
        private const val FILE = "sidecast_pairings"
        private const val KEY_PREFIX = "mac:"

        fun encodePairedMac(mac: PairedMac): String = JSONObject()
            .put("serverId", mac.serverId)
            .put("serverName", mac.serverName)
            .put("fingerprint", mac.fingerprintHex)
            .put("token", mac.tokenB64)
            .put("lastHost", mac.lastHost)
            .put("lastPort", mac.lastPort)
            .put("serverPort", mac.serverPort)
            .put(
                "addresses",
                JSONArray().apply { for (a in mac.addresses) put(JSONObject().put("host", a.host).put("kind", a.kind)) },
            )
            .put("platform", mac.platform)
            .toString()

        /**
         * Pairings stored before 0.4 have no addresses / serverPort: they read as empty / 0. Those
         * stored before hosts announced their OS have no platform: they were all Macs.
         */
        fun parsePairedMac(text: String): PairedMac? = try {
            val o = JSONObject(text)
            PairedMac(
                serverId = o.getString("serverId"),
                serverName = o.optString("serverName", ""),
                fingerprintHex = o.getString("fingerprint"),
                tokenB64 = o.getString("token"),
                lastHost = o.optString("lastHost", ""),
                lastPort = o.optInt("lastPort", 0),
                addresses = o.optJSONArray("addresses")?.let { array ->
                    (0 until array.length()).mapNotNull { i ->
                        val a = array.optJSONObject(i) ?: return@mapNotNull null
                        val host = a.optString("host", "").takeIf { it.isNotBlank() } ?: return@mapNotNull null
                        ServerAddress(host, a.optString("kind", ""))
                    }
                } ?: emptyList(),
                serverPort = o.optInt("serverPort", 0),
                platform = o.optString("platform", HostPlatform.MACOS),
            )
        } catch (_: JSONException) {
            null
        }
    }
}
