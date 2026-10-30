package com.achreftlili.sidecast.ui

import android.net.wifi.WifiManager
import android.os.Bundle
import android.view.ViewGroup
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.isVisible
import androidx.core.view.updateLayoutParams
import androidx.core.view.updatePadding
import androidx.recyclerview.widget.LinearLayoutManager
import com.achreftlili.sidecast.R
import com.achreftlili.sidecast.databinding.ActivityMainBinding
import com.achreftlili.sidecast.net.DiscoveredMac
import com.achreftlili.sidecast.net.MacDiscovery
import com.achreftlili.sidecast.store.AppSettings
import com.achreftlili.sidecast.store.PairingStore
import com.google.android.material.dialog.MaterialAlertDialogBuilder

/**
 * List of Macs (discovered + paired), "Add by IP address", long-press to forget; the overflow menu
 * holds the settings.
 */
class MainActivity : AppCompatActivity() {
    private lateinit var binding: ActivityMainBinding
    private lateinit var pairings: PairingStore
    private lateinit var settings: AppSettings
    private lateinit var discovery: MacDiscovery
    private lateinit var adapter: MacListAdapter
    private var discovered: List<DiscoveredMac> = emptyList()
    private var multicastLock: WifiManager.MulticastLock? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)
        pairings = PairingStore(this)
        settings = AppSettings(this)
        setUpMenu()

        adapter = MacListAdapter(onClick = ::open, onLongClick = ::confirmForget)
        binding.macList.layoutManager = LinearLayoutManager(this)
        binding.macList.adapter = adapter
        binding.addByIp.setOnClickListener {
            AddHostDialog.show(this) { host, port ->
                startActivity(SessionTarget(host, port, serverId = null, name = host).intent(this))
            }
        }
        discovery = MacDiscovery(this) {
            discovered = it
            refresh()
        }
        applyInsets()
    }

    override fun onStart() {
        super.onStart()
        refresh() // pairings may have changed in the display screen
        acquireMulticastLock()
        discovery.start()
    }

    override fun onStop() {
        discovery.stop()
        releaseMulticastLock()
        super.onStop()
    }

    private fun setUpMenu() {
        binding.toolbar.inflateMenu(R.menu.main)
        binding.toolbar.menu.findItem(R.id.action_mic_enabled).isChecked = settings.micEnabled
        binding.toolbar.menu.findItem(R.id.action_data_saver).isChecked = settings.dataSaverOnMobile
        binding.toolbar.setOnMenuItemClickListener { item ->
            when (item.itemId) {
                R.id.action_mic_enabled -> {
                    item.isChecked = !item.isChecked
                    settings.micEnabled = item.isChecked
                    true
                }
                R.id.action_data_saver -> {
                    item.isChecked = !item.isChecked
                    settings.dataSaverOnMobile = item.isChecked
                    true
                }
                else -> false
            }
        }
    }

    private fun refresh() {
        val items = MacList.merge(discovered, pairings.all())
        adapter.submitList(items)
        binding.emptyState.isVisible = items.isEmpty()
    }

    private fun open(item: MacListItem) {
        val host = item.host ?: return
        startActivity(SessionTarget(host, item.port, item.serverId, item.name, discovered = item.online).intent(this))
    }

    private fun confirmForget(item: MacListItem): Boolean {
        val serverId = item.serverId ?: return false
        if (!item.paired) return false
        MaterialAlertDialogBuilder(this)
            .setTitle(getString(R.string.forget_title, item.name))
            .setMessage(R.string.forget_message)
            .setPositiveButton(R.string.forget_confirm) { _, _ ->
                pairings.remove(serverId)
                refresh()
            }
            .setNegativeButton(R.string.action_cancel, null)
            .show()
        return true
    }

    /** Some Wi-Fi drivers drop multicast (mDNS) in power save unless an app holds this lock. */
    private fun acquireMulticastLock() {
        val wifi = applicationContext.getSystemService(WifiManager::class.java) ?: return
        val lock = multicastLock ?: wifi.createMulticastLock("Sidecast:discovery").also {
            it.setReferenceCounted(false)
            multicastLock = it
        }
        try {
            lock.acquire()
        } catch (_: SecurityException) {
        }
    }

    private fun releaseMulticastLock() {
        multicastLock?.takeIf { it.isHeld }?.release()
    }

    private fun applyInsets() {
        val listPadding = binding.macList.paddingStart
        val listBottom = binding.macList.paddingBottom
        val emptyPadding = binding.emptyState.paddingStart
        val fabMargin = (binding.addByIp.layoutParams as ViewGroup.MarginLayoutParams).bottomMargin
        ViewCompat.setOnApplyWindowInsetsListener(binding.root) { _, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            binding.appBar.updatePadding(left = bars.left, top = bars.top, right = bars.right)
            binding.macList.updatePadding(
                left = listPadding + bars.left,
                right = listPadding + bars.right,
                bottom = listBottom + bars.bottom,
            )
            binding.emptyState.updatePadding(left = emptyPadding + bars.left, right = emptyPadding + bars.right)
            binding.addByIp.updateLayoutParams<ViewGroup.MarginLayoutParams> {
                rightMargin = fabMargin + bars.right
                bottomMargin = fabMargin + bars.bottom
            }
            WindowInsetsCompat.CONSUMED
        }
    }
}
