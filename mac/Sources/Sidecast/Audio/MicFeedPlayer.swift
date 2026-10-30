import AudioToolbox
import CoreAudio
import Foundation
import os
import SidecastKit

/// `AudioJitterBuffer` shared between the network side (push) and the render
/// callback (read). The lock is an os_unfair_lock (priority-donating) held for
/// a copy of at most one render buffer; `read` never allocates.
final class SharedJitterBuffer: @unchecked Sendable {
    private let lock = OSAllocatedUnfairLock(uncheckedState: AudioJitterBuffer())

    func push(_ packet: AudioPacket) {
        lock.withLockUnchecked { $0.push(packet) }
    }

    func read(into out: UnsafeMutableBufferPointer<Float>) {
        lock.withLockUnchecked { _ = $0.read(into: out) }
    }

    func reset() {
        lock.withLockUnchecked { $0.reset() }
    }

    func takeStats() -> AudioJitterBuffer.Stats {
        lock.withLockUnchecked { $0.takeStats() }
    }
}

/// Plays the phone's audio into the hidden "Sidecast Mic Feed" device with an
/// AUHAL output unit (48 kHz mono float32, the device's own format); the
/// driver loops it back out of "Sidecast Microphone". Start/stop on one queue.
final class MicFeedPlayer {
    struct PlayerError: Error, CustomStringConvertible {
        let description: String
        init(_ what: String, _ status: OSStatus) { description = "\(what) failed (\(status))" }
    }

    let buffer: SharedJitterBuffer
    private var unit: AudioUnit?
    private(set) var device: AudioObjectID = kAudioObjectUnknown

    init(buffer: SharedJitterBuffer) {
        self.buffer = buffer
    }

    deinit { stop() }

    var isRunning: Bool { unit != nil }

    func start(device: AudioObjectID) throws {
        stop()
        var description = AudioComponentDescription(componentType: kAudioUnitType_Output,
                                                    componentSubType: kAudioUnitSubType_HALOutput,
                                                    componentManufacturer: kAudioUnitManufacturer_Apple,
                                                    componentFlags: 0, componentFlagsMask: 0)
        guard let component = AudioComponentFindNext(nil, &description) else { throw PlayerError("AUHAL lookup", -1) }
        var newUnit: AudioUnit?
        try check(AudioComponentInstanceNew(component, &newUnit), "AudioComponentInstanceNew")
        guard let unit = newUnit else { throw PlayerError("AudioComponentInstanceNew", -1) }
        do {
            var deviceID = device
            try check(AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                                           &deviceID, UInt32(MemoryLayout<AudioObjectID>.size)), "set device")
            var format = AudioStreamBasicDescription(
                mSampleRate: Double(AudioPacket.sampleRate), mFormatID: kAudioFormatLinearPCM,
                mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved,
                mBytesPerPacket: 4, mFramesPerPacket: 1, mBytesPerFrame: 4, mChannelsPerFrame: 1,
                mBitsPerChannel: 32, mReserved: 0)
            try check(AudioUnitSetProperty(unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0,
                                           &format, UInt32(MemoryLayout<AudioStreamBasicDescription>.size)), "set format")
            var callback = AURenderCallbackStruct(inputProc: micFeedRender,
                                                  inputProcRefCon: Unmanaged.passUnretained(buffer).toOpaque())
            try check(AudioUnitSetProperty(unit, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0,
                                           &callback, UInt32(MemoryLayout<AURenderCallbackStruct>.size)), "set callback")
            try check(AudioUnitInitialize(unit), "AudioUnitInitialize")
            try check(AudioOutputUnitStart(unit), "AudioOutputUnitStart")
        } catch {
            AudioUnitUninitialize(unit)
            AudioComponentInstanceDispose(unit)
            throw error
        }
        self.unit = unit
        self.device = device
    }

    func stop() {
        guard let unit else { return }
        self.unit = nil
        device = kAudioObjectUnknown
        AudioOutputUnitStop(unit)
        AudioUnitUninitialize(unit)
        AudioComponentInstanceDispose(unit)
    }

    private func check(_ status: OSStatus, _ what: String) throws {
        if status != noErr { throw PlayerError(what, status) }
    }
}

/// Render callback (real-time thread): copy from the jitter buffer, silence on underrun.
/// `buffer` outlives the unit (the player owns both).
private let micFeedRender: AURenderCallback = { refCon, _, _, _, frames, ioData in
    guard let ioData else { return noErr }
    let buffer = Unmanaged<SharedJitterBuffer>.fromOpaque(refCon).takeUnretainedValue()
    for audioBuffer in UnsafeMutableAudioBufferListPointer(ioData) {
        guard let data = audioBuffer.mData else { continue }
        let count = min(Int(frames), Int(audioBuffer.mDataByteSize) / MemoryLayout<Float>.size)
        buffer.read(into: UnsafeMutableBufferPointer(start: data.assumingMemoryBound(to: Float.self), count: count))
    }
    return noErr
}
