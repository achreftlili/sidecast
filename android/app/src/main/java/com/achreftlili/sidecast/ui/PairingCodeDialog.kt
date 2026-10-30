package com.achreftlili.sidecast.ui

import android.app.Activity
import android.content.DialogInterface
import android.os.Handler
import android.os.Looper
import android.view.WindowManager
import android.view.inputmethod.EditorInfo
import androidx.core.widget.doAfterTextChanged
import com.achreftlili.sidecast.R
import com.achreftlili.sidecast.databinding.DialogPairingCodeBinding
import com.achreftlili.sidecast.net.StreamClient.PairingPrompt
import com.achreftlili.sidecast.protocol.PairingCrypto
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import java.util.concurrent.TimeUnit

/**
 * "Enter the code shown on your Mac". Shows attempts left after a wrong code and counts down to
 * the code's expiry. Submits automatically once six digits are typed.
 */
class PairingCodeDialog(
    private val activity: Activity,
    macName: String,
    /** Returns true when the code was sent. */
    private val onSubmit: (String) -> Boolean,
    private val onCancel: () -> Unit,
    private val onExpired: () -> Unit,
) {
    private val binding = DialogPairingCodeBinding.inflate(activity.layoutInflater)
    private val handler = Handler(Looper.getMainLooper())
    private var prompt: PairingPrompt? = null
    private var suppressAutoSubmit = false

    private val dialog = MaterialAlertDialogBuilder(activity)
        .setTitle(activity.getString(R.string.pair_title, macName))
        .setView(binding.root)
        .setPositiveButton(R.string.pair_confirm, null)
        .setNegativeButton(R.string.action_cancel) { _, _ -> stopTicker(); onCancel() }
        .setCancelable(false)
        .create()

    private val ticker = object : Runnable {
        override fun run() {
            if (updateExpiry()) handler.postDelayed(this, 1_000)
        }
    }

    init {
        dialog.setOnShowListener {
            dialog.getButton(DialogInterface.BUTTON_POSITIVE).setOnClickListener { submit() }
            binding.code.requestFocus()
        }
        // Pan, never resize: in landscape the keyboard covers half the screen, and resizing
        // squeezes the code field to nothing, which moves focus to the Cancel button.
        dialog.window?.setSoftInputMode(
            WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_VISIBLE or WindowManager.LayoutParams.SOFT_INPUT_ADJUST_PAN,
        )
        binding.code.doAfterTextChanged { text ->
            if (suppressAutoSubmit) return@doAfterTextChanged
            binding.codeLayout.error = null
            if (text?.length == 6) submit()
        }
        binding.code.setOnEditorActionListener { _, actionId, _ ->
            if (actionId == EditorInfo.IME_ACTION_DONE) {
                submit()
                true
            } else {
                false
            }
        }
    }

    val isShowing: Boolean get() = dialog.isShowing

    fun show(prompt: PairingPrompt) {
        this.prompt = prompt
        suppressAutoSubmit = true
        binding.code.text?.clear()
        suppressAutoSubmit = false
        binding.codeLayout.error = if (prompt.wrongCode) {
            activity.resources.getQuantityString(R.plurals.pair_wrong_code, prompt.attemptsLeft, prompt.attemptsLeft)
        } else {
            null
        }
        if (!dialog.isShowing) dialog.show()
        stopTicker()
        handler.post(ticker)
    }

    fun dismiss() {
        stopTicker()
        if (dialog.isShowing) dialog.dismiss()
    }

    private fun submit() {
        val code = binding.code.text?.toString().orEmpty()
        if (!PairingCrypto.isValidCode(code)) {
            binding.codeLayout.error = activity.getString(R.string.pair_invalid)
            return
        }
        if (onSubmit(code)) dismiss()
    }

    /** False once expired (and the dialog was closed). */
    private fun updateExpiry(): Boolean {
        val p = prompt ?: return false
        val remaining = TimeUnit.NANOSECONDS.toSeconds(p.expiresAtNanos - System.nanoTime())
        if (remaining <= 0) {
            dismiss()
            onExpired()
            return false
        }
        binding.expiry.text = activity.getString(R.string.pair_expires_in, remaining / 60, remaining % 60)
        return true
    }

    private fun stopTicker() = handler.removeCallbacks(ticker)
}
