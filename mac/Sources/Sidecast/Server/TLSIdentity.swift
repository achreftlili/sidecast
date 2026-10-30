import Foundation
import Security
import SidecastKit

/// The Mac's self-signed EC P-256 TLS identity, generated once with
/// `/usr/bin/openssl` (LibreSSL) and loaded in memory only — nothing is
/// written to the keychain (`kSecImportToMemoryOnly`, macOS 15+).
struct TLSIdentity {
    let identity: SecIdentity
    let certificateDER: Data
    /// SHA-256 of the leaf certificate DER (PROTOCOL §2).
    let fingerprint: Data

    enum IdentityError: Error, CustomStringConvertible {
        case openssl(String)
        case importFailed(OSStatus)
        case noIdentity

        var description: String {
            switch self {
            case .openssl(let s): return "openssl failed: \(s)"
            case .importFailed(let s): return "SecPKCS12Import failed (\(s))"
            case .noIdentity: return "PKCS#12 contained no identity"
            }
        }
    }

    static let openssl = "/usr/bin/openssl"
    static let validityDays = 7300   // 20 years

    static func loadOrCreate(paths: AppPaths) throws -> TLSIdentity {
        let fm = FileManager.default
        if fm.fileExists(atPath: paths.identityP12.path), fm.fileExists(atPath: paths.identityPassword.path) {
            do {
                return try load(paths: paths)
            } catch {
                Log.error("TLS identity unreadable (\(error)); generating a new one — phones must pair again")
            }
        }
        // LibreSSL 3.3.6 writes an EC private scalar with a leading zero byte as a
        // 31-byte OCTET STRING (RFC 5915 wants 32); SecPKCS12Import then silently
        // yields no identity. That hits ~1 key in 256, so validate and retry.
        for attempt in 1...8 {
            try generate(paths: paths)
            do {
                return try load(paths: paths)
            } catch IdentityError.noIdentity {
                Log.info("generated key not importable (attempt \(attempt)); regenerating")
            }
        }
        throw IdentityError.noIdentity
    }

    static func load(paths: AppPaths) throws -> TLSIdentity {
        let p12 = try Data(contentsOf: paths.identityP12)
        let password = try String(contentsOf: paths.identityPassword, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let options: [String: Any] = [
            kSecImportExportPassphrase as String: password,
            kSecImportToMemoryOnly as String: true,
        ]
        var items: CFArray?
        let status = SecPKCS12Import(p12 as CFData, options as CFDictionary, &items)
        guard status == errSecSuccess else { throw IdentityError.importFailed(status) }
        guard let first = (items as? [[String: Any]])?.first,
              let value = first[kSecImportItemIdentity as String] else { throw IdentityError.noIdentity }
        let identity = value as! SecIdentity   // CF type: the cast cannot fail once present
        var cert: SecCertificate?
        guard SecIdentityCopyCertificate(identity, &cert) == errSecSuccess, let cert else {
            throw IdentityError.noIdentity
        }
        let der = SecCertificateCopyData(cert) as Data
        return TLSIdentity(identity: identity, certificateDER: der,
                           fingerprint: CertificateFingerprint.sha256(der: der))
    }

    /// Creates key + self-signed cert + PKCS#12 (random password) in a private temp dir.
    static func generate(paths: AppPaths) throws {
        try paths.ensureDirectory()
        let fm = FileManager.default
        let work = paths.dataDirectory.appendingPathComponent(".tls-\(UUID().uuidString)", isDirectory: true)
        try fm.createDirectory(at: work, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        defer { try? fm.removeItem(at: work) }

        let key = work.appendingPathComponent("key.pem").path
        let cert = work.appendingPathComponent("cert.pem").path
        let config = work.appendingPathComponent("req.cnf")
        let p12 = work.appendingPathComponent("identity.p12").path
        try Data("""
        [req]
        distinguished_name = dn
        x509_extensions = v3
        prompt = no
        [dn]
        CN = Sidecast
        [v3]
        basicConstraints = critical,CA:FALSE
        keyUsage = critical,digitalSignature
        extendedKeyUsage = serverAuth
        subjectKeyIdentifier = hash

        """.utf8).write(to: config)

        let password = SystemRandom().bytes(24).base64EncodedString()
        let env = ["PATH": "/usr/bin:/bin", "SIDECAST_P12_PASS": password]
        try runOpenSSL(["ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", key], env)
        try runOpenSSL(["req", "-new", "-x509", "-config", config.path, "-key", key, "-sha256",
                        "-days", String(validityDays), "-out", cert], env)
        // LibreSSL's default PBEs (RC2/3DES, SHA-1 MAC) are what SecPKCS12Import accepts.
        try runOpenSSL(["pkcs12", "-export", "-inkey", key, "-in", cert, "-name", "Sidecast",
                        "-passout", "env:SIDECAST_P12_PASS", "-out", p12], env)

        try SecureFile.write(try Data(contentsOf: URL(fileURLWithPath: p12)), to: paths.identityP12)
        try SecureFile.write(Data(password.utf8), to: paths.identityPassword)
        Log.info("generated a new TLS identity in \(paths.dataDirectory.path)")
    }

    private static func runOpenSSL(_ args: [String], _ env: [String: String]) throws {
        let r = ProcessRunner.run(openssl, args, environment: env, timeout: 30)
        guard r.succeeded else {
            throw IdentityError.openssl("\(args.first ?? ""): \(r.stderr.trimmingCharacters(in: .whitespacesAndNewlines))")
        }
    }
}
