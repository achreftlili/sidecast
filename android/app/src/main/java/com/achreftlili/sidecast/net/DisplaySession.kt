package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.net.StreamClient.EndReason
import com.achreftlili.sidecast.net.StreamClient.PairingPrompt
import com.achreftlili.sidecast.net.StreamClient.Phase
import com.achreftlili.sidecast.protocol.Audio
import com.achreftlili.sidecast.protocol.Cursor
import com.achreftlili.sidecast.protocol.InputModeName
import com.achreftlili.sidecast.protocol.MicControl
import com.achreftlili.sidecast.protocol.MicState
import com.achreftlili.sidecast.protocol.StreamConfig
import com.achreftlili.sidecast.protocol.Touch
import com.achreftlili.sidecast.protocol.VideoFrame
import com.achreftlili.sidecast.store.PairingRepository
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/** Receives the decoded-side of the stream. Called on the network reader thread, in order. */
interface VideoSink {
    fun onStreamConfig(config: StreamConfig)

    fun onVideoFrame(frame: VideoFrame, receivedAtNanos: Long)
}

/** Microphone requests of the stream (§7.2). Called on the session thread, in order. */
interface MicHandler {
    fun onMicControl(control: MicControl)

    /** The connection the microphone was serving ended; a new session starts with the mic off. */
    fun onMicSessionEnded()
}

/** What the display screen shows. */
sealed interface SessionState {
    data object Idle : SessionState

    data class Connecting(val macName: String) : SessionState

    /** PAIR_START sent; waiting for the Mac to show a code. */
    data class Pairing(val macName: String) : SessionState

    data class AwaitingCode(val macName: String, val prompt: PairingPrompt) : SessionState

    data class VerifyingCode(val macName: String) : SessionState

    data class Streaming(val macName: String) : SessionState

    data class Reconnecting(val macName: String, val cause: RetryCause, val attempt: Int) : SessionState

    data class Failed(val macName: String, val error: SessionError) : SessionState

    /** Closed because the app went to the background; [DisplaySession.resume] reconnects. */
    data object Paused : SessionState

    data object Stopped : SessionState
}

/**
 * Owns the reconnect loop around single-use [StreamClient]s and turns their events into
 * [SessionState]. State changes are serialized on one executor thread; video bypasses it and goes
 * straight from the reader thread to [videoSink].
 */
class DisplaySession(
    initialMacName: String,
    initialServerId: String?,
    private val pairings: PairingRepository,
    private val videoSink: VideoSink,
    private val clientFactory: ClientFactory,
    private val executor: ScheduledExecutorService =
        Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "sidecast-session").apply { isDaemon = true } },
) {
    fun interface ClientFactory {
        /** Builds a client for one attempt; pin the certificate of [expectedServerId] if paired. */
        fun create(expectedServerId: String?, counters: RxCounters, listener: StreamClient.Listener): StreamClient
    }

    private val _state = MutableStateFlow<SessionState>(SessionState.Idle)
    val state: StateFlow<SessionState> = _state.asStateFlow()

    private val _streamConfig = MutableStateFlow<StreamConfig?>(null)
    val streamConfig: StateFlow<StreamConfig?> = _streamConfig.asStateFlow()

    private val _cursor = MutableStateFlow<Cursor?>(null)

    /**
     * The Mac's pointer as last reported on the current connection (§7.3), latest value only (a
     * StateFlow conflates bursts); null until the first CURSOR after (re)connecting or a mode change.
     */
    val cursor: StateFlow<Cursor?> = _cursor.asStateFlow()

    private val _route = MutableStateFlow<Route?>(null)

    /** How the current (or currently attempted) connection reaches the Mac; null between attempts. */
    val route: StateFlow<Route?> = _route.asStateFlow()

    val counters = RxCounters()

    @Volatile
    private var client: StreamClient? = null

    /**
     * The user's input mode (§7.3). Read by each new client right after authentication (the
     * ClientFactory wires it), so every (re)connect announces it.
     */
    @Volatile
    var inputMode: String = InputModeName.TOUCH
        private set

    /** The user toggled touch / trackpad: remember it and tell the current stream. */
    fun setInputMode(mode: String) {
        inputMode = mode
        _cursor.value = null // the Mac sends a fresh position after the switch
        client?.sendInputMode(mode)
    }

    /** Receives MIC_CONTROL; set once before [start]. */
    @Volatile
    var micHandler: MicHandler? = null

    /** The client whose MIC_CONTROL the microphone is answering (MIC_STATE / AUDIO go there). */
    @Volatile
    private var micOwner: StreamClient? = null

    // Confined to the executor thread.
    private var macName = initialMacName
    private var expectedServerId = initialServerId
    private var identityChangedServerId: String? = null
    private var failures = 0
    private var everStreamed = false
    private var retryCause: RetryCause? = null
    private var pendingRetry: ScheduledFuture<*>? = null
    private var stopped = false

    fun start() = post {
        if (_state.value == SessionState.Idle) connect()
    }

    /** Terminal: closes everything; the session cannot be restarted. */
    fun stop() {
        post {
            stopped = true
            cancelRetry()
            val c = client
            client = null
            c?.close()
            _state.value = SessionState.Stopped
        }
        executor.shutdown()
    }

    /** User pressed Retry / Reconnect on an error. */
    fun retryNow() = post {
        if (stopped) return@post
        cancelRetry()
        client?.close()
        client = null
        failures = 0
        retryCause = null
        connect()
    }

    /** "Pair again" after an identity change: forget the stale pin and token, then reconnect. */
    fun forgetAndRepair() = post {
        if (stopped) return@post
        (identityChangedServerId ?: expectedServerId)?.let(pairings::remove)
        identityChangedServerId = null
        expectedServerId = null
        failures = 0
        retryCause = null
        cancelRetry()
        connect()
    }

    /** App went to the background for a while: drop the connection, keep the session. */
    fun pause() = post {
        if (stopped || _state.value is SessionState.Failed) return@post
        cancelRetry()
        val c = client
        client = null
        c?.close()
        _state.value = SessionState.Paused
    }

    fun resume() = post {
        if (stopped || _state.value != SessionState.Paused) return@post
        failures = 0
        retryCause = null
        connect()
    }

    /** The code's lifetime ran out on the phone before the Mac said so. */
    fun expirePairing() = post {
        if (_state.value !is SessionState.AwaitingCode) return@post
        val c = client
        client = null
        c?.close()
        _state.value = SessionState.Failed(macName, SessionError.PairingExpired)
    }

    /** Called on the UI thread; false when the code was not accepted for sending. */
    fun submitCode(code: String): Boolean = client?.submitPairingCode(code) ?: false

    fun sendTouch(touch: Touch) {
        client?.sendTouch(touch)
    }

    fun sendText(text: String) {
        client?.sendText(text)
    }

    fun sendKey(key: Int, modifiers: Int) {
        client?.sendKey(key, modifiers)
    }

    /** From any thread (the capture thread). */
    fun sendMicState(state: MicState) {
        micOwner?.sendMicState(state)
    }

    /** From the capture thread; never blocks. */
    fun sendAudio(audio: Audio) {
        micOwner?.sendAudio(audio)
    }

    fun discardPendingAudio() {
        micOwner?.discardPendingAudio()
    }

    fun requestKeyframe() {
        client?.requestKeyframe()
    }

    /** A connection attempt over [route] starts (called by the connector, any thread). */
    fun reportRoute(route: Route) {
        _route.value = route
    }

    // ------------------------------------------------------------------ internals

    private fun connect() {
        val listener = ClientListener()
        val c = try {
            clientFactory.create(expectedServerId, counters, listener)
        } catch (e: RuntimeException) {
            _state.value = SessionState.Failed(macName, SessionError.Protocol(e.message ?: e.javaClass.simpleName))
            return
        }
        listener.owner = c
        client = c
        val cause = retryCause
        _state.value = if (cause != null) {
            SessionState.Reconnecting(macName, cause, failures)
        } else {
            SessionState.Connecting(macName)
        }
        c.start()
    }

    private fun handleEnded(owner: StreamClient, reason: EndReason) {
        if (client !== owner) return
        client = null
        _route.value = null
        _cursor.value = null
        if (stopped) return
        if (reason is EndReason.IdentityChanged) identityChangedServerId = reason.serverId ?: expectedServerId
        when (val next = SessionPolicy.next(reason, everStreamed)) {
            NextStep.Stop -> _state.value = SessionState.Stopped
            is NextStep.Fail -> {
                retryCause = null
                _state.value = SessionState.Failed(macName, next.error)
            }
            is NextStep.Retry -> {
                retryCause = next.cause
                if (next.immediate) {
                    failures = 0
                    connect()
                } else {
                    failures++
                    _state.value = SessionState.Reconnecting(macName, next.cause, failures)
                    pendingRetry = executor.schedule(
                        { if (!stopped && client == null && _state.value is SessionState.Reconnecting) connect() },
                        ReconnectBackoff.delayMs(failures),
                        TimeUnit.MILLISECONDS,
                    )
                }
            }
        }
    }

    private fun handlePhase(phase: Phase) {
        when (phase) {
            Phase.PAIRING -> _state.value = SessionState.Pairing(macName)
            Phase.VERIFYING_CODE -> _state.value = SessionState.VerifyingCode(macName)
            Phase.STREAMING -> Unit // onAuthenticated
            Phase.CONNECTING, Phase.HANDSHAKING, Phase.AUTHENTICATING -> {
                if (_state.value !is SessionState.Reconnecting) _state.value = SessionState.Connecting(macName)
            }
        }
    }

    private fun cancelRetry() {
        pendingRetry?.cancel(false)
        pendingRetry = null
    }

    private fun post(block: () -> Unit) {
        try {
            executor.execute(block)
        } catch (_: java.util.concurrent.RejectedExecutionException) {
            // stopped
        }
    }

    private inner class ClientListener : StreamClient.Listener {
        @Volatile
        lateinit var owner: StreamClient

        private val isCurrent: Boolean get() = client === owner

        override fun onPhase(phase: Phase) = post { if (isCurrent) handlePhase(phase) }

        override fun onPairingCodeRequired(prompt: PairingPrompt) = post {
            if (isCurrent) _state.value = SessionState.AwaitingCode(macName, prompt)
        }

        override fun onAuthenticated(server: StreamClient.ServerIdentity) = post {
            if (!isCurrent) return@post
            failures = 0
            retryCause = null
            everStreamed = true
            expectedServerId = server.serverId
            if (server.serverName.isNotBlank()) macName = server.serverName
            _state.value = SessionState.Streaming(macName)
        }

        override fun onStreamConfig(config: StreamConfig) {
            if (!isCurrent) return
            _streamConfig.value = config
            videoSink.onStreamConfig(config)
        }

        override fun onVideoFrame(frame: VideoFrame, receivedAtNanos: Long) {
            if (isCurrent) videoSink.onVideoFrame(frame, receivedAtNanos)
        }

        override fun onCursor(cursor: Cursor) {
            if (isCurrent) _cursor.value = cursor
        }

        override fun onRoute(route: Route) {
            if (isCurrent) _route.value = route
        }

        override fun onMicControl(control: MicControl) = post {
            if (!isCurrent) return@post
            micOwner = owner
            micHandler?.onMicControl(control)
        }

        override fun onEnded(reason: EndReason) = post {
            if (micOwner === owner) {
                micOwner = null
                micHandler?.onMicSessionEnded()
            }
            handleEnded(owner, reason)
        }
    }
}
