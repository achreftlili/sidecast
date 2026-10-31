package com.achreftlili.sidecast.ui

import android.Manifest
import android.annotation.SuppressLint
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.Gravity
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.View
import android.view.ViewConfiguration
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.FrameLayout
import androidx.activity.addCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.core.view.isVisible
import androidx.core.view.updateLayoutParams
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import com.achreftlili.sidecast.R
import com.achreftlili.sidecast.databinding.ActivityDisplayBinding
import com.achreftlili.sidecast.input.KeyboardBridge
import com.achreftlili.sidecast.input.Letterbox
import com.achreftlili.sidecast.input.PointerPlacement
import com.achreftlili.sidecast.input.TouchForwarder
import com.achreftlili.sidecast.input.VideoRect
import com.achreftlili.sidecast.net.Route
import com.achreftlili.sidecast.net.SessionState
import com.achreftlili.sidecast.protocol.Cursor
import com.achreftlili.sidecast.protocol.InputModeName
import com.achreftlili.sidecast.protocol.StreamConfig
import com.achreftlili.sidecast.video.DecoderStatus
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.util.Locale

/**
 * Fullscreen extended display: immersive, landscape, screen kept on. Shows the stream letterboxed
 * in a SurfaceView, forwards touches and the phone's keyboard, and overlays connection / pairing /
 * error states.
 */
class DisplayActivity : AppCompatActivity(), OverlayActions {
    private lateinit var binding: ActivityDisplayBinding
    private lateinit var target: SessionTarget
    private lateinit var presenter: OverlayPresenter
    private val viewModel: DisplayViewModel by viewModels()

    private var state: SessionState = SessionState.Idle
    private var decoderStatus: DecoderStatus = DecoderStatus.Idle
    private var streamConfig: StreamConfig? = null
    private var route: Route? = null

    // The Mac's pointer (§7.3): latest CURSOR, applied at most once per frame.
    private var cursor: Cursor? = null
    private var playing = false
    private var pointerHotspotPx = 0f
    private var pointerScheduled = false
    private val applyPointer = Runnable {
        pointerScheduled = false
        renderPointer()
    }
    private var videoRect = VideoRect(0, 0, 0, 0)

    private var pairingDialog: PairingCodeDialog? = null
    private var shownPrompt: Any? = null
    private var disconnectDialog: AlertDialog? = null
    private var wifiLock: WifiManager.WifiLock? = null

    private lateinit var keyboardBridge: KeyboardBridge
    private lateinit var softKeyboard: SoftKeyboard
    private lateinit var keyboardFader: IdleFader
    private lateinit var inputModeFader: IdleFader
    private val hideInputModeLabel = Runnable {
        binding.inputModeLabel.animate().alpha(0f).setDuration(LABEL_FADE_MS).withEndAction {
            binding.inputModeLabel.isVisible = false
        }
    }

    // The answer goes to the ViewModel-scoped gate, so it survives activity recreation.
    private val micPermissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            viewModel.micPermission.onResult(granted)
        }

    // The whole screen is a remote pointer surface: there is no local "click" to perform.
    @SuppressLint("ClickableViewAccessibility")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        target = SessionTarget.from(intent) ?: run {
            finish()
            return
        }
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.attributes = window.attributes.apply {
            layoutInDisplayCutoutMode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
            } else {
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            }
        }
        WindowCompat.setDecorFitsSystemWindows(window, false)
        binding = ActivityDisplayBinding.inflate(layoutInflater)
        setContentView(binding.root)
        hideSystemBars()

        presenter = OverlayPresenter(this, target.name, this) { viewModel.session?.platform }
        viewModel.start(target, ScreenMetrics.landscape(this))
        val session = checkNotNull(viewModel.session)
        val pipeline = checkNotNull(viewModel.pipeline)
        val mic = checkNotNull(viewModel.mic)
        viewModel.micPermission.launcher = { micPermissionLauncher.launch(Manifest.permission.RECORD_AUDIO) }

        binding.surface.holder.addCallback(object : SurfaceHolder.Callback {
            override fun surfaceCreated(holder: SurfaceHolder) = pipeline.setSurface(holder.surface)

            override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) =
                pipeline.setSurface(holder.surface)

            override fun surfaceDestroyed(holder: SurfaceHolder) = pipeline.setSurface(null)
        })
        pointerHotspotPx = POINTER_HOTSPOT_DP * resources.displayMetrics.density
        setUpKeyboard()
        setUpInputMode()
        val edgeBandPx = EDGE_BAND_DP * resources.displayMetrics.density
        val controlSlopPx = CONTROL_SLOP_DP * resources.displayMetrics.density
        val touchForwarder = TouchForwarder(
            slopPx = ViewConfiguration.get(this).scaledTouchSlop.toFloat(),
            videoRect = { videoRect },
            send = viewModel::sendTouch,
            onThreeFingerTap = ::toggleStats,
            isIgnoredAt = { x, y, first -> isHeldBack(x, y, first, edgeBandPx, controlSlopPx) },
            log = { Log.d(TAG, it) },
        )
        binding.root.setOnTouchListener { view, event ->
            val handled = touchForwarder.onTouch(view, event) // forward first: nothing may delay it
            // A touch on the stream may click elsewhere on the Mac: typing context is stale.
            if (event.actionMasked == MotionEvent.ACTION_DOWN) binding.keyboardInput.onRemoteCursorMayHaveMoved()
            handled
        }
        binding.root.addOnLayoutChangeListener { _, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom ->
            if (right - left != oldRight - oldLeft || bottom - top != oldBottom - oldTop) binding.root.post(::layoutVideo)
        }
        applyCutoutInsets()
        onBackPressedDispatcher.addCallback(this) { confirmDisconnect() }

        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                launch {
                    session.state.collect {
                        state = it
                        render()
                    }
                }
                launch {
                    session.cursor.collect {
                        cursor = it
                        schedulePointer()
                    }
                }
                launch {
                    session.route.collect {
                        route = it
                        render()
                    }
                }
                launch {
                    session.streamConfig.collect {
                        streamConfig = it
                        layoutVideo()
                    }
                }
                launch {
                    pipeline.status.collect {
                        decoderStatus = it
                        render()
                    }
                }
                launch {
                    mic.capturing.collect { binding.micIndicator.isVisible = it }
                }
                launch {
                    while (true) {
                        val sample = viewModel.sampleStats()
                        if (binding.stats.isVisible && sample != null) binding.stats.text = formatStats(sample)
                        delay(1_000)
                    }
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        viewModel.onForeground()
        acquireWifiLock()
    }

    override fun onStop() {
        softKeyboard.hide()
        releaseWifiLock()
        viewModel.onStopped(finishing = isFinishing) // the microphone never records in the background
        super.onStop()
    }

    override fun onDestroy() {
        if (::binding.isInitialized) viewModel.micPermission.launcher = null
        pairingDialog?.dismiss()
        disconnectDialog?.dismiss()
        super.onDestroy()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) hideSystemBars()
    }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        val handled = super.dispatchTouchEvent(ev) // the touch is forwarded before any UI work
        if (ev.actionMasked == MotionEvent.ACTION_DOWN && ::softKeyboard.isInitialized) onUserTouch()
        return handled
    }

    /** Hardware-keyboard keys that no view consumed (the input view is not focused). */
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        (::keyboardBridge.isInitialized && keyboardBridge.onKeyDown(event)) || super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
        (::keyboardBridge.isInitialized && keyboardBridge.onKeyUp(event)) || super.onKeyUp(keyCode, event)

    // ------------------------------------------------------------------ OverlayActions

    override fun retry() {
        viewModel.session?.retryNow()
    }

    override fun pairAgain() {
        viewModel.session?.forgetAndRepair()
    }

    override fun close() = finish()

    // ------------------------------------------------------------------ rendering

    private fun render() {
        if (state == SessionState.Stopped) {
            finish()
            return
        }
        renderPairingDialog()
        val overlay = presenter.present(state, decoderStatus, route)
        playing = overlay == null && state is SessionState.Streaming
        renderControls(available = playing)
        schedulePointer()
        binding.overlay.isVisible = overlay != null
        binding.scrim.isVisible = overlay != null
        if (overlay == null) return
        binding.progress.isVisible = overlay.progress
        binding.title.text = overlay.title
        binding.detail.text = overlay.detail
        binding.detail.isVisible = overlay.detail != null
        bindAction(binding.primaryAction, overlay.primary)
        bindAction(binding.secondaryAction, overlay.secondary)
        binding.actions.isVisible = overlay.primary != null || overlay.secondary != null
    }

    /** Keyboard input only while the stream is up; otherwise keys keep their usual meaning. */
    /** Keyboard and input-mode buttons only while the stream is up; otherwise keys keep their usual meaning. */
    private fun renderControls(available: Boolean) {
        keyboardBridge.enabled = available
        if (binding.keyboardButton.isVisible == available) return
        binding.keyboardButton.isVisible = available
        binding.inputModeButton.isVisible = available
        if (available) {
            keyboardFader.poke()
            inputModeFader.poke()
        } else {
            keyboardFader.stop()
            inputModeFader.stop()
            softKeyboard.hide()
        }
    }

    private fun bindAction(button: com.google.android.material.button.MaterialButton, action: OverlayAction?) {
        button.isVisible = action != null
        if (action != null) {
            button.text = action.label
            button.setOnClickListener { action.onClick() }
        }
    }

    private fun renderPairingDialog() {
        val current = state
        if (current is SessionState.AwaitingCode) {
            if (shownPrompt == current.prompt && pairingDialog?.isShowing == true) return
            val dialog = pairingDialog ?: PairingCodeDialog(
                activity = this,
                macName = current.macName,
                onSubmit = { code -> viewModel.session?.submitCode(code) == true },
                onCancel = ::finish,
                onExpired = { viewModel.session?.expirePairing() },
            ).also { pairingDialog = it }
            shownPrompt = current.prompt
            dialog.show(current.prompt)
        } else {
            shownPrompt = null
            pairingDialog?.dismiss()
        }
    }

    /** Size the SurfaceView to the stream's aspect ratio, centered (letterbox / pillarbox). */
    private fun layoutVideo() {
        val config = streamConfig
        if (binding.root.width == 0 || binding.root.height == 0) return // not laid out yet
        val rect = Letterbox.fit(binding.root.width, binding.root.height, config?.width ?: 0, config?.height ?: 0)
        videoRect = rect
        schedulePointer()
        val lp = binding.surface.layoutParams as FrameLayout.LayoutParams
        val width = if (config == null) ViewGroup.LayoutParams.MATCH_PARENT else rect.width
        val height = if (config == null) ViewGroup.LayoutParams.MATCH_PARENT else rect.height
        if (lp.width != width || lp.height != height) {
            lp.width = width
            lp.height = height
            lp.gravity = Gravity.CENTER
            binding.surface.layoutParams = lp
        }
    }

    private fun toggleStats() {
        binding.stats.isVisible = !binding.stats.isVisible
        if (binding.stats.isVisible) binding.stats.text = getString(R.string.stats_hint)
    }

    private fun formatStats(s: StatsSnapshot): String {
        val config = streamConfig
        val rtt = s.rttMs?.let { getString(R.string.stats_rtt_ms, it.toInt()) } ?: getString(R.string.stats_rtt_unknown)
        val line = getString(
            R.string.stats_line,
            config?.codec ?: "-",
            config?.width ?: 0,
            config?.height ?: 0,
            config?.fps ?: 0,
            s.rxFps,
            String.format(Locale.ROOT, "%.1f", s.mbps),
            s.shownFps,
            rtt,
            s.dropped.toInt(),
        )
        // "Remote" whenever the stream does not come over the local network (Tailscale, internet).
        val remote = route != null && route != Route.LAN
        return if (remote) getString(R.string.stats_remote_line, getString(R.string.stats_remote), line) else line
    }

    private fun confirmDisconnect() {
        val current = state
        if (current is SessionState.Failed || current == SessionState.Stopped) {
            finish()
            return
        }
        val name = (current as? SessionState.Streaming)?.macName ?: target.name
        disconnectDialog?.dismiss()
        disconnectDialog = MaterialAlertDialogBuilder(this)
            .setTitle(getString(R.string.disconnect_title, name))
            .setPositiveButton(R.string.disconnect_confirm) { _, _ -> finish() }
            .setNegativeButton(R.string.action_cancel, null)
            .setOnDismissListener { hideSystemBars() }
            .show()
    }

    // ------------------------------------------------------------------ keyboard

    private fun setUpKeyboard() {
        keyboardBridge = KeyboardBridge(viewModel::sendKeyboard)
        binding.keyboardInput.bridge = keyboardBridge
        // The keyboard may have revealed the navigation bar: go back to immersive once it closes.
        softKeyboard = SoftKeyboard(binding.keyboardInput) { binding.root.post(::hideSystemBars) }
        binding.keyboardInput.onBackBeforeIme = softKeyboard::onBackToIme
        keyboardFader = IdleFader(binding.keyboardButton)
        // A clickable view consumes its own touches, so they never reach the TouchForwarder.
        binding.keyboardButton.setOnClickListener { softKeyboard.toggle() }
    }

    private fun onUserTouch() {
        if (binding.keyboardButton.isVisible) keyboardFader.poke()
        if (binding.inputModeButton.isVisible) inputModeFader.poke()
        if (!softKeyboard.isShown) hideSystemBars()
    }

    // ------------------------------------------------------------------ input mode (§7.3)

    private fun setUpInputMode() {
        inputModeFader = IdleFader(binding.inputModeButton)
        showInputMode(viewModel.inputMode)
        binding.inputModeButton.setOnClickListener {
            val next = if (viewModel.inputMode == InputModeName.TRACKPAD) InputModeName.TOUCH else InputModeName.TRACKPAD
            viewModel.setInputMode(next)
            showInputMode(next)
            flashInputModeLabel(next)
        }
    }

    /** Icon and description show the current mode; in trackpad mode the button never fades out. */
    private fun showInputMode(mode: String) {
        val trackpad = mode == InputModeName.TRACKPAD
        binding.inputModeButton.setImageResource(if (trackpad) R.drawable.ic_input_trackpad else R.drawable.ic_input_touch)
        binding.inputModeButton.contentDescription = getString(
            if (trackpad) R.string.input_mode_trackpad_description else R.string.input_mode_touch_description,
        )
        inputModeFader.idleAlpha = if (trackpad) ACTIVE_ALPHA else IDLE_ALPHA
        if (binding.inputModeButton.isVisible) inputModeFader.poke()
        schedulePointer()
    }

    // ------------------------------------------------------------------ pointer (§7.3)

    /** CURSOR can arrive at 60 Hz: coalesce to one placement per frame. */
    private fun schedulePointer() {
        if (pointerScheduled) return
        pointerScheduled = true
        binding.pointer.postOnAnimation(applyPointer)
    }

    /** Translation only (no layout pass); hidden via INVISIBLE, which does not relayout either. */
    private fun renderPointer() {
        val view = binding.pointer
        val position = PointerPlacement.position(cursor, viewModel.inputMode, playing, videoRect)
        if (position == null) {
            if (view.visibility != View.INVISIBLE) view.visibility = View.INVISIBLE
            return
        }
        view.translationX = position.x - pointerHotspotPx - view.left
        view.translationY = position.y - pointerHotspotPx - view.top
        if (view.visibility != View.VISIBLE) view.visibility = View.VISIBLE
    }

    private fun flashInputModeLabel(mode: String) {
        val label = binding.inputModeLabel
        label.text = getString(if (mode == InputModeName.TRACKPAD) R.string.input_mode_trackpad_label else R.string.input_mode_touch_label)
        label.removeCallbacks(hideInputModeLabel)
        label.animate().cancel()
        label.alpha = 1f
        label.isVisible = true
        label.postDelayed(hideInputModeLabel, LABEL_VISIBLE_MS)
    }

    /**
     * Pointers the Mac must not see (PointerFilter): any that goes down on a floating control,
     * and an extra one in the edge band, where a thumb holding the phone rests.
     */
    private fun isHeldBack(x: Float, y: Float, first: Boolean, edgeBandPx: Float, controlSlopPx: Float): Boolean {
        val controls = listOf(binding.keyboardButton, binding.inputModeButton, binding.micIndicator, binding.inputModeLabel)
        for (view in controls) {
            if (!view.isVisible) continue
            val left = view.left + view.translationX - controlSlopPx
            val top = view.top + view.translationY - controlSlopPx
            if (x >= left && x <= left + view.width + 2 * controlSlopPx && y >= top && y <= top + view.height + 2 * controlSlopPx) {
                return true
            }
        }
        if (first) return false
        val root = binding.root
        return x < edgeBandPx || y < edgeBandPx || x > root.width - edgeBandPx || y > root.height - edgeBandPx
    }

    // ------------------------------------------------------------------ window

    private fun hideSystemBars() {
        val controller = WindowCompat.getInsetsController(window, window.decorView)
        controller.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller.hide(WindowInsetsCompat.Type.systemBars())
    }

    /**
     * Keep text and buttons out of the camera cutout; the video itself uses the whole panel. The
     * keyboard button also rides above the soft keyboard when the platform reports its insets
     * (adjustNothing: the stream itself never moves).
     */
    private fun applyCutoutInsets() {
        val statsMargin = (binding.stats.layoutParams as ViewGroup.MarginLayoutParams).leftMargin
        val keyboardMargin = (binding.keyboardButton.layoutParams as ViewGroup.MarginLayoutParams).bottomMargin
        val micMarginEnd = (binding.micIndicator.layoutParams as ViewGroup.MarginLayoutParams).marginEnd
        val micMarginBottom = (binding.micIndicator.layoutParams as ViewGroup.MarginLayoutParams).bottomMargin
        val modeMarginEnd = (binding.inputModeButton.layoutParams as ViewGroup.MarginLayoutParams).marginEnd
        val labelMarginBottom = (binding.inputModeLabel.layoutParams as ViewGroup.MarginLayoutParams).bottomMargin
        ViewCompat.setOnApplyWindowInsetsListener(binding.root) { _, insets ->
            val cutout = insets.getInsets(WindowInsetsCompat.Type.displayCutout())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            val endCutout = if (binding.root.layoutDirection == View.LAYOUT_DIRECTION_RTL) cutout.left else cutout.right
            binding.keyboardButton.updateLayoutParams<ViewGroup.MarginLayoutParams> {
                marginEnd = keyboardMargin + endCutout
                bottomMargin = keyboardMargin + maxOf(cutout.bottom, ime.bottom)
            }
            binding.inputModeButton.updateLayoutParams<ViewGroup.MarginLayoutParams> {
                marginEnd = modeMarginEnd + endCutout
                bottomMargin = keyboardMargin + maxOf(cutout.bottom, ime.bottom)
            }
            binding.inputModeLabel.updateLayoutParams<ViewGroup.MarginLayoutParams> {
                bottomMargin = labelMarginBottom + maxOf(cutout.bottom, ime.bottom)
            }
            binding.micIndicator.updateLayoutParams<ViewGroup.MarginLayoutParams> {
                marginEnd = micMarginEnd + endCutout
                bottomMargin = micMarginBottom + maxOf(cutout.bottom, ime.bottom)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                softKeyboard.onImeInsets(insets.isVisible(WindowInsetsCompat.Type.ime()))
            }
            binding.stats.updateLayoutParams<ViewGroup.MarginLayoutParams> {
                leftMargin = statsMargin + cutout.left
                topMargin = statsMargin + cutout.top
            }
            binding.overlay.updateLayoutParams<ViewGroup.MarginLayoutParams> {
                leftMargin = cutout.left
                rightMargin = cutout.right
            }
            WindowInsetsCompat.CONSUMED
        }
    }

    /** Low-latency Wi-Fi mode (no power-save batching) while the display is visible. */
    private fun acquireWifiLock() {
        val wifi = applicationContext.getSystemService(WifiManager::class.java) ?: return
        val lock = wifiLock ?: wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "Sidecast:stream").also {
            it.setReferenceCounted(false)
            wifiLock = it
        }
        try {
            lock.acquire()
        } catch (_: SecurityException) {
        }
    }

    private fun releaseWifiLock() {
        wifiLock?.takeIf { it.isHeld }?.release()
    }

    private companion object {
        const val TAG = "Sidecast/Touch"

        /** Extra (non-first) pointers starting this close to a screen edge are a gripping thumb. */
        const val EDGE_BAND_DP = 16f

        /** Margin around floating controls inside which a pointer never reaches the Mac. */
        const val CONTROL_SLOP_DP = 4f

        const val ACTIVE_ALPHA = 0.5f
        const val IDLE_ALPHA = 0.25f
        const val LABEL_VISIBLE_MS = 1_500L
        const val LABEL_FADE_MS = 200L

        /** The arrow tip inside ic_pointer (1 viewport unit = 1 dp). */
        const val POINTER_HOTSPOT_DP = 3f
    }
}
