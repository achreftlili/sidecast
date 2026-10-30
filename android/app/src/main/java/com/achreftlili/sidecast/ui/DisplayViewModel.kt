package com.achreftlili.sidecast.ui

import android.app.Application
import android.net.ConnectivityManager
import android.os.Process
import android.util.Log
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.achreftlili.sidecast.audio.AndroidMicRecorder
import com.achreftlili.sidecast.audio.MicSink
import com.achreftlili.sidecast.audio.MicStreamer
import com.achreftlili.sidecast.input.KeyboardEvent
import com.achreftlili.sidecast.net.ConnectPlanner
import com.achreftlili.sidecast.net.DataSaver
import com.achreftlili.sidecast.net.DisplaySession
import com.achreftlili.sidecast.net.FallbackConnector
import com.achreftlili.sidecast.net.LanSocketBinder
import com.achreftlili.sidecast.net.StreamClient
import com.achreftlili.sidecast.net.TlsConnector
import com.achreftlili.sidecast.protocol.Audio
import com.achreftlili.sidecast.protocol.Feature
import com.achreftlili.sidecast.protocol.Hello
import com.achreftlili.sidecast.protocol.InputModeName
import com.achreftlili.sidecast.protocol.MicState
import com.achreftlili.sidecast.protocol.ScreenInfo
import com.achreftlili.sidecast.protocol.Touch
import com.achreftlili.sidecast.store.AppSettings
import com.achreftlili.sidecast.store.DeviceIdentity
import com.achreftlili.sidecast.store.PairingStore
import com.achreftlili.sidecast.video.DecoderCapabilities
import com.achreftlili.sidecast.video.VideoPipeline
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/** Rates over the last sampling interval, for the stats overlay. */
data class StatsSnapshot(
    val rxFps: Int,
    val mbps: Double,
    val shownFps: Int,
    val rttMs: Long?,
    val dropped: Long,
)

/**
 * Owns the [DisplaySession], [VideoPipeline] and [MicStreamer] so a stream survives activity
 * recreation. Built once per display screen via [start].
 */
class DisplayViewModel(application: Application) : AndroidViewModel(application) {
    private val pairings = PairingStore(application)
    private val settings = AppSettings(application)

    /** RECORD_AUDIO requests; the visible activity installs the dialog launcher. */
    internal val micPermission = MicPermissionGate(application)

    var session: DisplaySession? = null
        private set
    var pipeline: VideoPipeline? = null
        private set
    var mic: MicStreamer? = null
        private set

    private var pauseJob: Job? = null

    private var lastSampleNanos = 0L
    private var lastFrames = 0L
    private var lastBytes = 0L
    private var lastRendered = 0L

    fun start(target: SessionTarget, screen: ScreenInfo) {
        if (session != null) return
        val app = getApplication<Application>()
        val binder = LanSocketBinder(app)
        // Codec probing enumerates MediaCodecList: evaluated lazily on the session thread.
        val hello by lazy {
            Hello(
                deviceId = DeviceIdentity.deviceId(app),
                deviceName = DeviceIdentity.deviceName(app),
                screen = screen,
                codecs = DecoderCapabilities.advertisedCodecs(screen),
                // Always advertised; the phone setting is enforced by answering MIC_STATE disabled.
                features = listOf(Feature.MIC),
            )
        }
        val pipeline = VideoPipeline(operatingRate = screen.refreshRate.roundToInt().coerceAtLeast(60)) {
            session?.requestKeyframe()
        }
        val connectivity = app.getSystemService(ConnectivityManager::class.java)
        // A Bonjour address is only "current" for the first attempt; later attempts (reconnects,
        // e.g. after moving from Wi-Fi to mobile data) run the full plan (§1.1).
        var bonjourHost = target.host.takeIf { target.discovered }
        val factory = DisplaySession.ClientFactory { expectedServerId, counters, listener ->
            val paired = expectedServerId?.let(pairings::get)
            val pin = paired?.fingerprint // every candidate is pinned: the token never goes elsewhere
            val plan = ConnectPlanner.plan(target.host, target.port, bonjourHost, paired)
            bonjourHost = null
            Log.d(TAG, "connect plan: $plan")
            val connector = FallbackConnector(
                candidates = plan,
                connectorFor = { c ->
                    TlsConnector(c.host, c.port, pin, prepareSocket = binder::bind, connectTimeoutMs = c.connectTimeoutMs)
                },
                onAttempt = { c -> session?.reportRoute(c.route) },
            )
            // Evaluated per connection: the phone may have moved to (or off) mobile data.
            val metered = connectivity?.isActiveNetworkMetered ?: false
            StreamClient(
                connector = connector,
                hello = DataSaver.apply(hello, metered, settings.dataSaverOnMobile),
                pairings = pairings,
                endpoint = StreamClient.Endpoint(target.host, target.port),
                listener = listener,
                counters = counters,
                threadSetup = ::raiseThreadPriority,
                log = { Log.d(TAG, it) },
                inputMode = { session?.inputMode ?: InputModeName.TOUCH },
            )
        }
        this.pipeline = pipeline
        val session = DisplaySession(target.name, target.serverId, pairings, pipeline, factory)
        val mic = MicStreamer(
            sink = object : MicSink {
                override fun sendMicState(state: MicState) = session.sendMicState(state)

                override fun sendAudio(audio: Audio) = session.sendAudio(audio)

                override fun discardPendingAudio() = session.discardPendingAudio()
            },
            recorders = AndroidMicRecorder.factory(app),
            permission = micPermission,
            isEnabled = { settings.micEnabled },
            threadSetup = { setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO) },
            log = { Log.d(MIC_TAG, it) },
        )
        session.micHandler = mic
        session.setInputMode(settings.inputMode) // announced after every authentication
        this.mic = mic
        this.session = session
        session.start()
    }

    fun sendTouch(touch: Touch) {
        session?.sendTouch(touch)
    }

    /** The current touch / trackpad mode (§7.3). */
    val inputMode: String get() = session?.inputMode ?: settings.inputMode

    /** The user toggled the input mode: persist it and tell the Mac. */
    fun setInputMode(mode: String) {
        settings.inputMode = mode
        session?.setInputMode(mode)
    }

    fun sendKeyboard(event: KeyboardEvent) {
        val s = session ?: return
        when (event) {
            is KeyboardEvent.Type -> s.sendText(event.text)
            is KeyboardEvent.Press -> s.sendKey(event.key, event.modifiers)
        }
    }

    /** Activity started: cancel a pending background pause, reconnect if it already happened. */
    fun onForeground() {
        pauseJob?.cancel()
        pauseJob = null
        session?.resume()
        mic?.onForeground()
    }

    /**
     * Activity stopped (screen off, app switch, or finishing): the microphone stops at once; the
     * stream is kept briefly, then let go.
     */
    fun onStopped(finishing: Boolean) {
        mic?.onBackground()
        if (!finishing) onBackground()
    }

    private fun onBackground() {
        pauseJob?.cancel()
        pauseJob = viewModelScope.launch {
            delay(BACKGROUND_GRACE_MS)
            session?.pause()
        }
    }

    fun sampleStats(): StatsSnapshot? {
        val s = session ?: return null
        val p = pipeline ?: return null
        val now = System.nanoTime()
        val frames = s.counters.videoFrames.get()
        val bytes = s.counters.videoBytes.get()
        val rendered = p.stats.rendered.get()
        val previous = lastSampleNanos
        val seconds = (now - previous) / 1e9
        val snapshot = if (previous != 0L && seconds > 0) {
            StatsSnapshot(
                rxFps = ((frames - lastFrames) / seconds).roundToInt(),
                mbps = (bytes - lastBytes) * 8 / seconds / 1e6,
                shownFps = ((rendered - lastRendered) / seconds).roundToInt(),
                rttMs = s.counters.rttMicros.takeIf { it >= 0 }?.let { (it + 500) / 1000 },
                dropped = p.stats.droppedInput.get() + p.stats.droppedLate.get(),
            )
        } else {
            null
        }
        lastSampleNanos = now
        lastFrames = frames
        lastBytes = bytes
        lastRendered = rendered
        return snapshot
    }

    override fun onCleared() {
        mic?.release()
        session?.stop()
        pipeline?.release()
    }

    private fun raiseThreadPriority() = setThreadPriority(Process.THREAD_PRIORITY_DISPLAY)

    private fun setThreadPriority(priority: Int) {
        try {
            Process.setThreadPriority(priority)
        } catch (e: RuntimeException) {
            Log.w(TAG, "setThreadPriority failed: ${e.message}")
        }
    }

    private companion object {
        const val TAG = "Sidecast/Session"
        const val MIC_TAG = "Sidecast/Mic"
        const val BACKGROUND_GRACE_MS = 30_000L
    }
}
