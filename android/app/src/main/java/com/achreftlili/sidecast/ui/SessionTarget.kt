package com.achreftlili.sidecast.ui

import android.content.Context
import android.content.Intent

/** Which Mac the display screen connects to (passed as Intent extras). */
data class SessionTarget(
    val host: String,
    val port: Int,
    /** Known when picked from the list; null for "Add by IP address" (learned from SERVER_HELLO). */
    val serverId: String?,
    val name: String,
    /** [host] is the Mac's Bonjour address, resolved just now on the local network. */
    val discovered: Boolean = false,
) {
    fun intent(context: Context): Intent = Intent(context, DisplayActivity::class.java)
        .putExtra(EXTRA_HOST, host)
        .putExtra(EXTRA_PORT, port)
        .putExtra(EXTRA_SERVER_ID, serverId)
        .putExtra(EXTRA_NAME, name)
        .putExtra(EXTRA_DISCOVERED, discovered)

    companion object {
        private const val EXTRA_HOST = "com.achreftlili.sidecast.HOST"
        private const val EXTRA_PORT = "com.achreftlili.sidecast.PORT"
        private const val EXTRA_SERVER_ID = "com.achreftlili.sidecast.SERVER_ID"
        private const val EXTRA_NAME = "com.achreftlili.sidecast.NAME"
        private const val EXTRA_DISCOVERED = "com.achreftlili.sidecast.DISCOVERED"

        fun from(intent: Intent): SessionTarget? {
            val host = intent.getStringExtra(EXTRA_HOST) ?: return null
            val port = intent.getIntExtra(EXTRA_PORT, 0).takeIf { it in 1..65535 } ?: return null
            return SessionTarget(
                host = host,
                port = port,
                serverId = intent.getStringExtra(EXTRA_SERVER_ID),
                name = intent.getStringExtra(EXTRA_NAME) ?: host,
                discovered = intent.getBooleanExtra(EXTRA_DISCOVERED, false),
            )
        }
    }
}
