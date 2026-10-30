package com.achreftlili.sidecast.ui

import android.app.Activity
import android.content.DialogInterface
import android.view.WindowManager
import android.view.inputmethod.EditorInfo
import androidx.core.widget.doAfterTextChanged
import com.achreftlili.sidecast.R
import com.achreftlili.sidecast.databinding.DialogAddHostBinding
import com.achreftlili.sidecast.protocol.Protocol
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import java.util.Locale

/** "Add by IP address": host + port (default 47800). */
object AddHostDialog {
    fun show(activity: Activity, onConnect: (host: String, port: Int) -> Unit) {
        val binding = DialogAddHostBinding.inflate(activity.layoutInflater)
        binding.port.setText(String.format(Locale.ROOT, "%d", Protocol.DEFAULT_PORT))
        val dialog = MaterialAlertDialogBuilder(activity)
            .setTitle(R.string.add_title)
            .setView(binding.root)
            .setPositiveButton(R.string.add_connect, null)
            .setNegativeButton(R.string.action_cancel, null)
            .create()

        fun submit() {
            val parsed = HostInput.parse(binding.host.text?.toString().orEmpty(), binding.port.text?.toString().orEmpty())
            if (parsed == null) {
                val portOk = binding.port.text.isNullOrBlank() ||
                    binding.port.text.toString().toIntOrNull()?.let { it in 1..65535 } == true
                if (!portOk) {
                    binding.portLayout.error = activity.getString(R.string.add_error_port)
                } else {
                    binding.hostLayout.error = activity.getString(R.string.add_error_host)
                }
                return
            }
            dialog.dismiss()
            onConnect(parsed.host, parsed.port)
        }

        binding.host.doAfterTextChanged { binding.hostLayout.error = null }
        binding.port.doAfterTextChanged { binding.portLayout.error = null }
        binding.port.setOnEditorActionListener { _, actionId, _ ->
            if (actionId == EditorInfo.IME_ACTION_DONE) {
                submit()
                true
            } else {
                false
            }
        }
        dialog.setOnShowListener {
            dialog.getButton(DialogInterface.BUTTON_POSITIVE).setOnClickListener { submit() }
            binding.host.requestFocus()
        }
        dialog.window?.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE)
        dialog.show()
    }
}
