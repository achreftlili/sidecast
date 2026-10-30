import CoreAudio
import Foundation

/// Small, synchronous Core Audio HAL helpers (client side). Hidden devices
/// such as "Sidecast Mic Feed" are not in the device list; they are found by UID.
enum CoreAudioDevices {
    static func address(_ selector: AudioObjectPropertySelector,
                        _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }

    /// The device with this UID, or nil if it does not exist (right now).
    static func device(forUID uid: String) -> AudioObjectID? {
        var a = address(kAudioHardwarePropertyTranslateUIDToDevice)
        var cfUID = uid as CFString
        var device = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        let status = withUnsafeMutablePointer(to: &cfUID) { q in
            AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a,
                                       UInt32(MemoryLayout<CFString>.size), q, &size, &device)
        }
        return status == noErr && device != kAudioObjectUnknown ? device : nil
    }

    static func uid(of device: AudioObjectID) -> String? {
        var a = address(kAudioDevicePropertyDeviceUID)
        var uid: Unmanaged<CFString>?
        var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(device, &a, 0, nil, &size, &uid) == noErr, let uid else { return nil }
        return uid.takeRetainedValue() as String
    }

    static func defaultInputDevice() -> AudioObjectID? {
        var a = address(kAudioHardwarePropertyDefaultInputDevice)
        var device = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        let status = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil, &size, &device)
        return status == noErr && device != kAudioObjectUnknown ? device : nil
    }

    @discardableResult
    static func setDefaultInputDevice(_ device: AudioObjectID) -> OSStatus {
        var a = address(kAudioHardwarePropertyDefaultInputDevice)
        var value = device
        return AudioObjectSetPropertyData(AudioObjectID(kAudioObjectSystemObject), &a, 0, nil,
                                          UInt32(MemoryLayout<AudioObjectID>.size), &value)
    }

    /// `kAudioDevicePropertyDeviceIsRunningSomewhere`: some process does IO on the device.
    static func isRunningSomewhere(_ device: AudioObjectID) -> Bool {
        var a = address(kAudioDevicePropertyDeviceIsRunningSomewhere)
        var running: UInt32 = 0
        var size = UInt32(MemoryLayout<UInt32>.size)
        return AudioObjectGetPropertyData(device, &a, 0, nil, &size, &running) == noErr && running != 0
    }
}
