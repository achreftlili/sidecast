import CoreAudio
import CoreAudio.AudioServerPlugIn
import SidecastMicDriver
import XCTest

/// Drives SidecastMic.driver's AudioServerPlugIn interface in-process, the
/// way coreaudiod would, with a stub host. Nothing touches the real Core Audio.
/// `plugInTypeUUID` / `driverInterfaceUUID` are C macros Swift cannot import.
private let plugInTypeUUID = CFUUIDGetConstantUUIDWithBytes(
    nil, 0x44, 0x3A, 0xBA, 0xB8, 0xE7, 0xB3, 0x49, 0x1A, 0xB9, 0x85, 0xBE, 0xB9, 0x18, 0x70, 0x30, 0xDB)
private let driverInterfaceUUID = CFUUIDGetConstantUUIDWithBytes(
    nil, 0xEE, 0xA5, 0x77, 0x3D, 0xCC, 0x43, 0x49, 0xF1, 0x8E, 0x00, 0x8F, 0x96, 0xE7, 0xD2, 0x3B, 0x17)
private let opReadInput = AudioServerPlugInIOOperation.readInput.rawValue
private let opWriteMix = AudioServerPlugInIOOperation.writeMix.rawValue

final class DriverTests: XCTestCase {
    private static var host = AudioServerPlugInHostInterface(
        PropertiesChanged: { _, _, _, _ in noErr },
        CopyFromStorage: { _, _, out in out.pointee = nil; return noErr },
        WriteToStorage: { _, _, _ in noErr },
        DeleteFromStorage: { _, _ in noErr },
        RequestDeviceConfigurationChange: { _, _, _, _ in noErr })

    private let plugIn = AudioObjectID(kAudioObjectPlugInObject)
    private let input = AudioObjectID(SCM_OBJECT_INPUT_DEVICE)
    private let inputStream = AudioObjectID(SCM_OBJECT_INPUT_STREAM)
    private let feed = AudioObjectID(SCM_OBJECT_FEED_DEVICE)
    private let feedStream = AudioObjectID(SCM_OBJECT_FEED_STREAM)

    private var driver: AudioServerPlugInDriverRef!
    private var api: AudioServerPlugInDriverInterface { driver.pointee!.pointee }

    override func setUpWithError() throws {
        let raw = try XCTUnwrap(SidecastMic_Create(kCFAllocatorDefault, plugInTypeUUID))
        driver = raw.assumingMemoryBound(to: UnsafeMutablePointer<AudioServerPlugInDriverInterface>?.self)
        XCTAssertEqual(withUnsafePointer(to: &Self.host) { api.Initialize(driver, $0) }, noErr)
    }

    // MARK: - Helpers

    private func address(_ selector: AudioObjectPropertySelector,
                         _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }

    private func size(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                      _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws -> UInt32 {
        var a = address(selector, scope)
        var size: UInt32 = 0
        let status = api.GetPropertyDataSize(driver, object, 0, &a, 0, nil, &size)
        guard status == noErr else { throw NSError(domain: "driver", code: Int(status)) }
        return size
    }

    private func value<T>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                          _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal,
                          qualifier: UnsafeRawPointer? = nil, qualifierSize: UInt32 = 0,
                          as type: T.Type = T.self) throws -> T {
        var a = address(selector, scope)
        let storage = UnsafeMutableRawPointer.allocate(byteCount: max(MemoryLayout<T>.size, 8),
                                                       alignment: MemoryLayout<T>.alignment)
        defer { storage.deallocate() }
        var outSize: UInt32 = 0
        let status = api.GetPropertyData(driver, object, 0, &a, qualifierSize, qualifier,
                                         UInt32(MemoryLayout<T>.size), &outSize, storage)
        guard status == noErr else { throw NSError(domain: "driver", code: Int(status)) }
        XCTAssertEqual(outSize, UInt32(MemoryLayout<T>.size))
        return storage.load(as: T.self)
    }

    private func string(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) throws -> String {
        let s: Unmanaged<CFString> = try value(object, selector)
        return s.takeUnretainedValue() as String
    }

    private func ids(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                     _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws -> [AudioObjectID] {
        let bytes = try size(object, selector, scope)
        var result = [AudioObjectID](repeating: 0, count: Int(bytes) / MemoryLayout<AudioObjectID>.size)
        var a = address(selector, scope)
        var outSize: UInt32 = 0
        let status = result.withUnsafeMutableBytes { buffer in
            api.GetPropertyData(driver, object, 0, &a, 0, nil, bytes, &outSize, buffer.baseAddress!)
        }
        XCTAssertEqual(status, noErr)
        XCTAssertEqual(outSize, bytes)
        return result
    }

    private func translate(_ uid: String) throws -> AudioObjectID {
        var cf = uid as CFString
        return try withUnsafePointer(to: &cf) { q in
            try value(plugIn, kAudioPlugInPropertyTranslateUIDToDevice, qualifier: UnsafeRawPointer(q),
                      qualifierSize: UInt32(MemoryLayout<CFString>.size), as: AudioObjectID.self)
        }
    }

    private func has(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> Bool {
        var a = address(selector)
        return api.HasProperty(driver, object, 0, &a).boolValue
    }

    // MARK: - Factory and plug-in

    func testFactoryAndQueryInterface() {
        XCTAssertNil(SidecastMic_Create(kCFAllocatorDefault, CFUUIDCreate(nil)))
        let iid = CFUUIDGetUUIDBytes(driverInterfaceUUID)
        var out: LPVOID?
        XCTAssertEqual(api.QueryInterface(driver, iid, &out), S_OK)
        XCTAssertEqual(out, UnsafeMutableRawPointer(driver))
        _ = api.Release(driver)
        let bogus = CFUUIDGetUUIDBytes(CFUUIDCreate(nil))
        XCTAssertEqual(api.QueryInterface(driver, bogus, &out), HRESULT(bitPattern: 0x8000_0004))   // E_NOINTERFACE
    }

    func testPlugInListsBothDevicesAndTranslatesUIDs() throws {
        XCTAssertEqual(try ids(plugIn, kAudioPlugInPropertyDeviceList), [input, feed])
        XCTAssertEqual(try ids(plugIn, kAudioPlugInPropertyBoxList), [])
        XCTAssertEqual(try value(plugIn, kAudioObjectPropertyClass, as: AudioClassID.self), kAudioPlugInClassID)
        XCTAssertEqual(try translate("SidecastMic_Input"), input)
        XCTAssertEqual(try translate("SidecastMic_Feed"), feed)
        XCTAssertEqual(try translate("Something_Else"), kAudioObjectUnknown)
        XCTAssertEqual(try string(plugIn, kAudioObjectPropertyManufacturer), "Sidecast")
    }

    // MARK: - Devices

    func testDeviceIdentity() throws {
        XCTAssertEqual(try string(input, kAudioObjectPropertyName), "Sidecast Microphone")
        XCTAssertEqual(try string(feed, kAudioObjectPropertyName), "Sidecast Mic Feed")
        XCTAssertEqual(try string(input, kAudioDevicePropertyDeviceUID), "SidecastMic_Input")
        XCTAssertEqual(try string(feed, kAudioDevicePropertyDeviceUID), "SidecastMic_Feed")
        for device in [input, feed] {
            XCTAssertEqual(try value(device, kAudioObjectPropertyClass, as: AudioClassID.self), kAudioDeviceClassID)
            XCTAssertEqual(try value(device, kAudioObjectPropertyOwner, as: AudioObjectID.self), plugIn)
            XCTAssertEqual(try value(device, kAudioDevicePropertyTransportType, as: UInt32.self),
                           kAudioDeviceTransportTypeVirtual)
            XCTAssertEqual(try value(device, kAudioDevicePropertyDeviceIsAlive, as: UInt32.self), 1)
            XCTAssertEqual(try value(device, kAudioDevicePropertyNominalSampleRate, as: Float64.self), 48000)
            let range: AudioValueRange = try value(device, kAudioDevicePropertyAvailableNominalSampleRates)
            XCTAssertEqual(range.mMinimum, 48000)
            XCTAssertEqual(range.mMaximum, 48000)
            XCTAssertEqual(try value(device, kAudioDevicePropertyZeroTimeStampPeriod, as: UInt32.self), 16384)
            XCTAssertEqual(try ids(device, kAudioObjectPropertyControlList), [])
        }
        // Visible microphone that can be the default input; hidden feed that cannot be a default.
        XCTAssertEqual(try value(input, kAudioDevicePropertyIsHidden, as: UInt32.self), 0)
        XCTAssertEqual(try value(feed, kAudioDevicePropertyIsHidden, as: UInt32.self), 1)
        XCTAssertEqual(try value(input, kAudioDevicePropertyDeviceCanBeDefaultDevice, kAudioObjectPropertyScopeInput,
                                 as: UInt32.self), 1)
        XCTAssertEqual(try value(feed, kAudioDevicePropertyDeviceCanBeDefaultDevice, kAudioObjectPropertyScopeOutput,
                                 as: UInt32.self), 0)
        XCTAssertEqual(try value(input, kAudioDevicePropertyDeviceCanBeDefaultSystemDevice, as: UInt32.self), 0)
        // One clock for both.
        let domain: UInt32 = try value(input, kAudioDevicePropertyClockDomain)
        XCTAssertNotEqual(domain, 0)
        XCTAssertEqual(try value(feed, kAudioDevicePropertyClockDomain, as: UInt32.self), domain)
    }

    func testStreamsLiveInTheirDirectionScope() throws {
        XCTAssertEqual(try ids(input, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput), [inputStream])
        XCTAssertEqual(try ids(input, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput), [])
        XCTAssertEqual(try ids(feed, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput), [feedStream])
        XCTAssertEqual(try ids(feed, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput), [])
        XCTAssertEqual(try value(inputStream, kAudioStreamPropertyDirection, as: UInt32.self), 1)
        XCTAssertEqual(try value(feedStream, kAudioStreamPropertyDirection, as: UInt32.self), 0)
        XCTAssertEqual(try value(inputStream, kAudioStreamPropertyTerminalType, as: UInt32.self),
                       kAudioStreamTerminalTypeMicrophone)
        XCTAssertEqual(try value(inputStream, kAudioObjectPropertyOwner, as: AudioObjectID.self), input)
        XCTAssertEqual(try value(feedStream, kAudioObjectPropertyOwner, as: AudioObjectID.self), feed)
    }

    func testStreamFormatIs48kMonoFloat() throws {
        for stream in [inputStream, feedStream] {
            for selector in [kAudioStreamPropertyVirtualFormat, kAudioStreamPropertyPhysicalFormat] {
                let f: AudioStreamBasicDescription = try value(stream, selector)
                XCTAssertEqual(f.mSampleRate, 48000)
                XCTAssertEqual(f.mFormatID, kAudioFormatLinearPCM)
                XCTAssertEqual(f.mFormatFlags, kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked)
                XCTAssertEqual(f.mChannelsPerFrame, 1)
                XCTAssertEqual(f.mBitsPerChannel, 32)
                XCTAssertEqual(f.mBytesPerFrame, 4)
            }
            XCTAssertEqual(try size(stream, kAudioStreamPropertyAvailableVirtualFormats),
                           UInt32(MemoryLayout<AudioStreamRangedDescription>.size))
        }
    }

    func testOnlyOurFormatAndRateCanBeSet() throws {
        var a = address(kAudioDevicePropertyNominalSampleRate)
        var rate: Float64 = 48000
        XCTAssertEqual(api.SetPropertyData(driver, input, 0, &a, 0, nil, 8, &rate), noErr)
        rate = 44100
        XCTAssertNotEqual(api.SetPropertyData(driver, input, 0, &a, 0, nil, 8, &rate), noErr)

        var f: AudioStreamBasicDescription = try value(inputStream, kAudioStreamPropertyVirtualFormat)
        var fa = address(kAudioStreamPropertyVirtualFormat)
        let asbdSize = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        XCTAssertEqual(api.SetPropertyData(driver, inputStream, 0, &fa, 0, nil, asbdSize, &f), noErr)
        f.mChannelsPerFrame = 2
        f.mBytesPerFrame = 8
        f.mBytesPerPacket = 8
        XCTAssertEqual(api.SetPropertyData(driver, inputStream, 0, &fa, 0, nil, asbdSize, &f),
                       kAudioDeviceUnsupportedFormatError)

        var settable: DarwinBoolean = false
        XCTAssertEqual(api.IsPropertySettable(driver, input, 0, &a, &settable), noErr)
        XCTAssertTrue(settable.boolValue)
        var nameAddress = address(kAudioObjectPropertyName)
        XCTAssertEqual(api.IsPropertySettable(driver, input, 0, &nameAddress, &settable), noErr)
        XCTAssertFalse(settable.boolValue)
    }

    func testUnknownPropertiesAndObjects() {
        XCTAssertFalse(has(input, 0x7a7a7a7a))
        XCTAssertFalse(has(99, kAudioObjectPropertyName))
        XCTAssertTrue(has(input, kAudioDevicePropertyDeviceIsRunningSomewhere))
        var a = address(kAudioObjectPropertyName)
        var size: UInt32 = 0
        XCTAssertEqual(api.GetPropertyDataSize(driver, 99, 0, &a, 0, nil, &size), kAudioHardwareBadObjectError)
    }

    // MARK: - IO

    func testRunningStateFollowsStartAndStop() throws {
        func running(_ d: AudioObjectID) throws -> (UInt32, UInt32) {
            (try value(d, kAudioDevicePropertyDeviceIsRunning), try value(d, kAudioDevicePropertyDeviceIsRunningSomewhere))
        }
        XCTAssertTrue(try running(input) == (0, 0))
        XCTAssertEqual(api.StartIO(driver, input, 1), noErr)
        XCTAssertEqual(api.StartIO(driver, input, 2), noErr)
        XCTAssertTrue(try running(input) == (1, 1))
        XCTAssertTrue(try running(feed) == (0, 0), "devices run independently")
        XCTAssertEqual(api.StopIO(driver, input, 1), noErr)
        XCTAssertTrue(try running(input) == (1, 1), "still running for the second client")
        XCTAssertEqual(api.StopIO(driver, input, 2), noErr)
        XCTAssertEqual(api.StopIO(driver, input, 2), noErr)   // extra stop must not underflow
        XCTAssertTrue(try running(input) == (0, 0))
        XCTAssertEqual(api.StartIO(driver, input, 3), noErr)
        XCTAssertTrue(try running(input) == (1, 1))
        XCTAssertEqual(api.StopIO(driver, input, 3), noErr)
        XCTAssertEqual(api.StartIO(driver, 99, 1), kAudioHardwareBadObjectError)
    }

    func testBothDevicesShareOneClock() {
        var s1: Float64 = -1, s2: Float64 = -1
        var h1: UInt64 = 0, h2: UInt64 = 0
        var seed1: UInt64 = 0, seed2: UInt64 = 0
        XCTAssertEqual(api.GetZeroTimeStamp(driver, input, 0, &s1, &h1, &seed1), noErr)
        XCTAssertEqual(api.GetZeroTimeStamp(driver, feed, 0, &s2, &h2, &seed2), noErr)
        XCTAssertEqual(s1, s2)
        XCTAssertEqual(h1, h2)
        XCTAssertEqual(seed1, seed2)
        XCTAssertEqual(s1.truncatingRemainder(dividingBy: 16384), 0)
        XCTAssertLessThanOrEqual(h1, mach_absolute_time())
        // A period later (16384 frames ≈ 341 ms) the clock has advanced by exactly one period.
        Thread.sleep(forTimeInterval: 0.36)
        var s3: Float64 = 0
        var h3: UInt64 = 0
        XCTAssertEqual(api.GetZeroTimeStamp(driver, input, 0, &s3, &h3, &seed1), noErr)
        XCTAssertTrue(s3 == s1 + 16384 || s3 == s1 + 32768, "\(s1) → \(s3)")
        var timebase = mach_timebase_info_data_t()
        mach_timebase_info(&timebase)
        let ns = Double(h3 - h1) * Double(timebase.numer) / Double(timebase.denom)
        XCTAssertEqual(ns / 1e9, (s3 - s1) / 48000, accuracy: 1e-6)
    }

    func testFeedComesOutOfTheMicrophone() {
        var willDo: DarwinBoolean = false, inPlace: DarwinBoolean = false
        XCTAssertEqual(api.WillDoIOOperation(driver, input, 0, opReadInput, &willDo, &inPlace), noErr)
        XCTAssertTrue(willDo.boolValue)
        XCTAssertTrue(inPlace.boolValue)
        XCTAssertEqual(api.WillDoIOOperation(driver, input, 0, opWriteMix, &willDo, &inPlace), noErr)
        XCTAssertFalse(willDo.boolValue)
        XCTAssertEqual(api.WillDoIOOperation(driver, feed, 0, opWriteMix, &willDo, &inPlace), noErr)
        XCTAssertTrue(willDo.boolValue)
        XCTAssertEqual(api.WillDoIOOperation(driver, feed, 0, opReadInput, &willDo, &inPlace), noErr)
        XCTAssertFalse(willDo.boolValue)

        let frames: UInt32 = 512
        let t: Float64 = 1_000_000
        var sine = (0..<Int(frames)).map { Float(sin(2 * Double.pi * 440 * Double($0) / 48000)) }
        var cycle = AudioServerPlugInIOCycleInfo()
        cycle.mOutputTime.mSampleTime = t
        let writeOp = opWriteMix, readOp = opReadInput
        XCTAssertEqual(api.DoIOOperation(driver, feed, feedStream, 0, writeOp, frames, &cycle, &sine, nil), noErr)

        func readInput(at sampleTime: Float64) -> [Float] {
            var buffer = [Float](repeating: .nan, count: Int(frames))
            var c = AudioServerPlugInIOCycleInfo()
            c.mInputTime.mSampleTime = sampleTime
            XCTAssertEqual(api.DoIOOperation(driver, input, inputStream, 0, readOp, frames, &c, &buffer, nil), noErr)
            return buffer
        }
        XCTAssertEqual(readInput(at: t), sine)
        let silence = [Float](repeating: 0, count: Int(frames))
        XCTAssertEqual(readInput(at: t + Float64(frames)), silence, "nothing fed there")
        XCTAssertEqual(readInput(at: t + Float64(SCM_RING_FRAMES)), silence, "a lap later the old audio is not replayed")
        // The input stream object must match the device.
        var buffer = [Float](repeating: 0, count: Int(frames))
        XCTAssertEqual(api.DoIOOperation(driver, input, feedStream, 0, readOp, frames, &cycle, &buffer, nil),
                       kAudioHardwareBadObjectError)
    }
}
