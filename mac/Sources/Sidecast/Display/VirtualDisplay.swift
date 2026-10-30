import AppKit
import CoreGraphics
import CVirtualDisplay
import SidecastKit

/// An extended virtual display backed by the private CGVirtualDisplay API
/// (creation ported from SideScreen's VirtualDisplayManager). The display
/// exists while this object lives. Main thread only.
final class VirtualDisplay {
    /// "SC". Also used to tell our displays apart from physical ones.
    static let vendorID: UInt32 = 0x5343

    enum DisplayError: Error, CustomStringConvertible {
        case creationFailed
        case settingsRejected

        var description: String {
            switch self {
            case .creationFailed: return "CGVirtualDisplay could not be created"
            case .settingsRejected: return "CGVirtualDisplay rejected the display modes"
            }
        }
    }

    let mode: VirtualDisplayMode
    private var display: CGVirtualDisplay?
    private var screenObserver: NSObjectProtocol?

    var displayID: CGDirectDisplayID { display?.displayID ?? kCGNullDirectDisplay }

    /// - Parameters:
    ///   - serial: distinguishes phones, so macOS remembers an arrangement per phone.
    init(mode: VirtualDisplayMode, name: String, serial: UInt32) throws {
        dispatchPrecondition(condition: .onQueue(.main))
        self.mode = mode
        let pixelW = mode.pixelWidth, pixelH = mode.pixelHeight

        let descriptor = CGVirtualDisplayDescriptor()
        descriptor.name = name
        descriptor.maxPixelsWide = UInt32(pixelW)
        descriptor.maxPixelsHigh = UInt32(pixelH)
        // macOS treats ≥ ~200 PPI as Retina; 110 PPI is a plain 1× panel.
        let ppi = mode.hiDPI ? 220.0 : 110.0
        descriptor.sizeInMillimeters = CGSize(width: Double(pixelW) * 25.4 / ppi, height: Double(pixelH) * 25.4 / ppi)
        descriptor.vendorID = Self.vendorID
        // width·10000 + height keeps 3840×2400 and 2400×3840 apart (SideScreen).
        descriptor.productID = UInt32(truncatingIfNeeded: pixelW * 10_000 + pixelH)
        descriptor.serialNum = serial

        let settings = CGVirtualDisplaySettings()
        settings.hiDPI = mode.hiDPI ? 1 : 0
        var modes: [CGVirtualDisplayMode] = []
        if mode.hiDPI {
            // The full-pixel "anchor" mode tells macOS the panel is high density,
            // which unlocks the 2× mode at the logical size.
            modes.append(CGVirtualDisplayMode(width: UInt32(pixelW), height: UInt32(pixelH),
                                              refreshRate: Double(mode.refreshRate)))
        }
        modes.append(CGVirtualDisplayMode(width: UInt32(mode.width), height: UInt32(mode.height),
                                          refreshRate: Double(mode.refreshRate)))
        settings.modes = modes

        guard let display = CGVirtualDisplay(descriptor: descriptor) else { throw DisplayError.creationFailed }
        self.display = display
        guard display.apply(settings) else {
            self.display = nil
            throw DisplayError.settingsRejected
        }
        Log.info("display: created \(mode.width)x\(mode.height) pt\(mode.hiDPI ? " HiDPI (\(pixelW)x\(pixelH) px)" : "") "
                 + "@\(mode.refreshRate) Hz, id \(display.displayID)")
        ensurePhysicalDisplayStaysMain()
        screenObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.ensurePhysicalDisplayStaysMain()
        }
    }

    deinit {
        if let screenObserver { NotificationCenter.default.removeObserver(screenObserver) }
        if display != nil { Log.info("display: destroyed") }
        display = nil
    }

    /// With HiDPI, makes sure the 2× mode (logical size, double pixels) is the one in use.
    func ensurePreferredMode() {
        let id = displayID
        guard mode.hiDPI, id != kCGNullDirectDisplay else { return }
        if let current = CGDisplayCopyDisplayMode(id),
           current.width == mode.width, current.pixelWidth == mode.pixelWidth { return }
        let options = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
        guard let all = CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode],
              let wanted = all.first(where: { $0.width == mode.width && $0.height == mode.height
                  && $0.pixelWidth == mode.pixelWidth && $0.pixelHeight == mode.pixelHeight }) else {
            Log.info("display: HiDPI mode not offered; keeping the current mode")
            return
        }
        var config: CGDisplayConfigRef?
        guard CGBeginDisplayConfiguration(&config) == .success, let config else { return }
        guard CGConfigureDisplayWithDisplayMode(config, id, wanted, nil) == .success else {
            CGCancelDisplayConfiguration(config)
            return
        }
        if CGCompleteDisplayConfiguration(config, .forSession) == .success {
            Log.info("display: switched to the HiDPI mode")
        }
    }

    /// Safety net (SideScreen #39): while a physical display is online, the
    /// main slot must never belong to the virtual display — otherwise the menu
    /// bar, Dock and keyboard focus land on a screen nobody is looking at.
    func ensurePhysicalDisplayStaysMain() {
        let id = displayID
        guard id != kCGNullDirectDisplay, CGMainDisplayID() == id,
              let physical = Self.onlinePhysicalDisplays(excluding: id).first else { return }
        var config: CGDisplayConfigRef?
        guard CGBeginDisplayConfiguration(&config) == .success, let config else { return }
        let physicalWidth = Int32(CGDisplayBounds(physical).width)
        var result = CGConfigureDisplayOrigin(config, physical, 0, 0)
        if result == .success { result = CGConfigureDisplayOrigin(config, id, physicalWidth, 0) }
        guard result == .success else {
            CGCancelDisplayConfiguration(config)
            Log.error("display: could not restore the physical main display (\(result.rawValue))")
            return
        }
        // Session-scoped: never bake the virtual display into the permanent arrangement.
        if CGCompleteDisplayConfiguration(config, .forSession) == .success {
            Log.info("display: physical display restored as main; virtual display parked beside it")
        }
    }

    /// Online displays that are not ours (vendor 0x5343) — a stale display of a
    /// previous run must not count as physical.
    static func onlinePhysicalDisplays(excluding id: CGDirectDisplayID) -> [CGDirectDisplayID] {
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        var count: UInt32 = 0
        guard CGGetOnlineDisplayList(16, &ids, &count) == .success else { return [] }
        return ids.prefix(Int(count)).filter { $0 != id && CGDisplayVendorNumber($0) != vendorID }
    }

    /// Stable 32-bit serial for a phone (FNV-1a of its deviceId).
    static func serial(for deviceId: String) -> UInt32 {
        var hash: UInt32 = 0x811C_9DC5
        for byte in deviceId.utf8 {
            hash ^= UInt32(byte)
            hash = hash &* 0x0100_0193
        }
        return hash == 0 ? 1 : hash
    }
}
