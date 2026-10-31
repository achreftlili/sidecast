package com.achreftlili.sidecast.ui

import android.content.res.ColorStateList
import android.view.LayoutInflater
import android.view.ViewGroup
import androidx.core.content.ContextCompat
import androidx.core.view.isVisible
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView
import com.achreftlili.sidecast.R
import com.achreftlili.sidecast.databinding.ItemMacBinding
import com.google.android.material.color.MaterialColors

class MacListAdapter(
    private val onClick: (MacListItem) -> Unit,
    private val onLongClick: (MacListItem) -> Boolean,
) : ListAdapter<MacListItem, MacListAdapter.Holder>(Diff) {

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder =
        Holder(ItemMacBinding.inflate(LayoutInflater.from(parent.context), parent, false))

    override fun onBindViewHolder(holder: Holder, position: Int) = holder.bind(getItem(position))

    inner class Holder(private val binding: ItemMacBinding) : RecyclerView.ViewHolder(binding.root) {
        fun bind(item: MacListItem) {
            val context = binding.root.context
            binding.name.text = item.name
            val status = when {
                item.online && !item.compatible ->
                    context.getString(R.string.mac_status_online_version, item.protocolVersion)
                item.online -> context.getString(R.string.mac_status_online, item.host)
                item.remote -> context.getString(R.string.mac_status_remote_tailscale)
                item.host != null -> context.getString(R.string.mac_status_offline, item.host)
                else -> context.getString(R.string.mac_status_offline_unknown)
            }
            // An OS this version does not know shows no label, like a generic computer.
            binding.status.text = PlatformLabel.of(item.platform)?.let {
                context.getString(R.string.mac_status_with_platform, context.getString(it), status)
            } ?: status
            val dotColor = if (item.online) {
                ContextCompat.getColor(context, R.color.status_online)
            } else {
                MaterialColors.getColor(binding.root, com.google.android.material.R.attr.colorOutline)
            }
            binding.statusDot.backgroundTintList = ColorStateList.valueOf(dotColor)
            binding.pairedBadge.isVisible = item.paired
            binding.root.alpha = if (item.online || item.host != null) 1f else 0.6f
            binding.root.setOnClickListener { onClick(item) }
            binding.root.setOnLongClickListener { onLongClick(item) }
        }
    }

    private object Diff : DiffUtil.ItemCallback<MacListItem>() {
        override fun areItemsTheSame(oldItem: MacListItem, newItem: MacListItem) = oldItem.key == newItem.key

        override fun areContentsTheSame(oldItem: MacListItem, newItem: MacListItem) = oldItem == newItem
    }
}
