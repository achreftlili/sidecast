import Foundation
import Security

/// Generic-password Keychain items of this app. Stored "after first unlock, this
/// device only": never synced to iCloud or restored to another device, and kept
/// across reinstalls (a free Apple ID needs one every 7 days) for the same
/// team and bundle identifier.
enum Keychain {
    struct Failure: Error, CustomStringConvertible {
        let status: OSStatus
        var description: String { "Keychain error \(status)" }
    }

    static func read(service: String, account: String) -> Data? {
        var query = base(service: service)
        query[kSecAttrAccount as String] = account
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess else { return nil }
        return result as? Data
    }

    /// Every item of `service`, by account.
    static func all(service: String) -> [String: Data] {
        var query = base(service: service)
        query[kSecReturnData as String] = true
        query[kSecReturnAttributes as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let items = result as? [[String: Any]] else { return [:] }
        var out: [String: Data] = [:]
        for item in items {
            if let account = item[kSecAttrAccount as String] as? String, let data = item[kSecValueData as String] as? Data {
                out[account] = data
            }
        }
        return out
    }

    static func write(_ data: Data, service: String, account: String) throws {
        var query = base(service: service)
        query[kSecAttrAccount as String] = account
        let update: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        var status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            query.merge(update) { $1 }
            status = SecItemAdd(query as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw Failure(status: status) }
    }

    static func delete(service: String, account: String? = nil) {
        var query = base(service: service)
        if let account { query[kSecAttrAccount as String] = account }
        SecItemDelete(query as CFDictionary)
    }

    private static func base(service: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrSynchronizable as String: false,
        ]
    }
}
