import XCTest
@testable import SidecastKit

final class PairedDeviceStoreTests: XCTestCase {
    private var dir: URL!

    override func setUpWithError() throws {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent("sidecast-store-\(UUID().uuidString)")
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    func testAddAuthenticatePersistAndForget() throws {
        let url = dir.appendingPathComponent("nested/paired-devices.json")
        let token = Data(repeating: 0xAA, count: 32)
        let store = PairedDeviceStore(fileURL: url)
        XCTAssertTrue(store.all.isEmpty)
        try store.add(deviceId: "dev-1", deviceName: "Pixel 8", token: token)

        XCTAssertEqual(store.authenticate(deviceId: "dev-1", token: token)?.deviceName, "Pixel 8")
        XCTAssertNil(store.authenticate(deviceId: "dev-1", token: Data(repeating: 0xAB, count: 32)))
        XCTAssertNil(store.authenticate(deviceId: "dev-2", token: token))

        // Only the hash is on disk, with owner-only permissions.
        let raw = try String(contentsOf: url, encoding: .utf8)
        XCTAssertFalse(raw.contains(token.base64EncodedString()))
        XCTAssertTrue(raw.contains(DeviceToken.hash(token).base64EncodedString()))
        let perms = try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as? Int
        XCTAssertEqual(perms, 0o600)

        // Reload from disk.
        let reloaded = PairedDeviceStore(fileURL: url)
        XCTAssertEqual(reloaded.all.map(\.deviceId), ["dev-1"])
        XCTAssertNotNil(reloaded.authenticate(deviceId: "dev-1", token: token))

        try reloaded.remove(deviceId: "dev-1")
        XCTAssertNil(PairedDeviceStore(fileURL: url).authenticate(deviceId: "dev-1", token: token))
    }

    func testRepairingReplacesTheToken() throws {
        let store = PairedDeviceStore(fileURL: dir.appendingPathComponent("p.json"))
        let old = Data(repeating: 1, count: 32), new = Data(repeating: 2, count: 32)
        try store.add(deviceId: "d", deviceName: "A", token: old)
        try store.add(deviceId: "d", deviceName: "B", token: new)
        XCTAssertEqual(store.all.count, 1)
        XCTAssertNil(store.authenticate(deviceId: "d", token: old))
        XCTAssertEqual(store.authenticate(deviceId: "d", token: new)?.deviceName, "B")
    }

    func testCorruptFileIsMovedAside() throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent("p.json")
        try Data("not json".utf8).write(to: url)
        let store = PairedDeviceStore(fileURL: url)
        XCTAssertTrue(store.all.isEmpty)
        XCTAssertTrue(FileManager.default.fileExists(atPath: url.path + ".corrupt"))
    }

    func testMarkSeenUpdatesName() throws {
        let store = PairedDeviceStore(fileURL: dir.appendingPathComponent("p.json"))
        try store.add(deviceId: "d", deviceName: "Old", token: Data(count: 32))
        store.markSeen(deviceId: "d", name: "New name")
        XCTAssertEqual(store.device(id: "d")?.deviceName, "New name")
    }
}
