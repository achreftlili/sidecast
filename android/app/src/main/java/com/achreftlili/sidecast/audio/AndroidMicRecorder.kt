package com.achreftlili.sidecast.audio

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.AudioEffect
import android.media.audiofx.AutomaticGainControl
import android.media.audiofx.NoiseSuppressor
import android.util.Log
import androidx.core.content.ContextCompat
import com.achreftlili.sidecast.protocol.Protocol

/**
 * [MicRecorder] over AudioRecord: voice-communication source (the platform's echo cancellation /
 * noise suppression path), 48 kHz mono PCM16, plus the AEC / NS / AGC effects where available.
 */
internal class AndroidMicRecorder private constructor(
    private val record: AudioRecord,
    private val effects: List<AudioEffect>,
) : MicRecorder {
    override fun start(): Boolean = try {
        record.startRecording()
        record.recordingState == AudioRecord.RECORDSTATE_RECORDING
    } catch (e: IllegalStateException) {
        Log.w(TAG, "startRecording failed: ${e.message}")
        false
    }

    override fun read(buffer: ShortArray): Int = record.read(buffer, 0, buffer.size)

    override fun release() {
        try {
            if (record.recordingState == AudioRecord.RECORDSTATE_RECORDING) record.stop()
        } catch (_: IllegalStateException) {
        }
        for (effect in effects) effect.release()
        record.release()
    }

    companion object {
        private const val TAG = "Sidecast/Mic"

        /** 100 ms of headroom in the platform buffer (reads are 10 ms). */
        private const val BUFFER_BYTES = Protocol.MIC_SAMPLE_RATE / 10 * 2

        fun factory(context: Context): MicRecorderFactory {
            val app = context.applicationContext
            return MicRecorderFactory { create(app) }
        }

        private fun create(context: Context): MicRecorder? {
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) !=
                PackageManager.PERMISSION_GRANTED
            ) {
                return null
            }
            val minBuffer = AudioRecord.getMinBufferSize(
                Protocol.MIC_SAMPLE_RATE,
                AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
            )
            if (minBuffer <= 0) return null
            val record = try {
                AudioRecord.Builder()
                    .setAudioSource(MediaRecorder.AudioSource.VOICE_COMMUNICATION)
                    .setAudioFormat(
                        AudioFormat.Builder()
                            .setSampleRate(Protocol.MIC_SAMPLE_RATE)
                            .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                            .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                            .build(),
                    )
                    .setBufferSizeInBytes(maxOf(minBuffer, BUFFER_BYTES))
                    .build()
            } catch (e: UnsupportedOperationException) {
                Log.w(TAG, "AudioRecord unsupported: ${e.message}")
                return null
            } catch (e: SecurityException) {
                Log.w(TAG, "AudioRecord refused: ${e.message}")
                return null
            }
            if (record.state != AudioRecord.STATE_INITIALIZED) {
                record.release()
                return null
            }
            return AndroidMicRecorder(record, enableEffects(record.audioSessionId))
        }

        private fun enableEffects(session: Int): List<AudioEffect> {
            val effects = ArrayList<AudioEffect>(3)
            try {
                if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(session)?.let(effects::add)
                if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(session)?.let(effects::add)
                if (AutomaticGainControl.isAvailable()) AutomaticGainControl.create(session)?.let(effects::add)
                for (effect in effects) effect.setEnabled(true)
            } catch (e: RuntimeException) {
                // Optional: the voice-communication source already applies its own processing.
                Log.w(TAG, "audio effects unavailable: ${e.message}")
            }
            return effects
        }
    }
}
