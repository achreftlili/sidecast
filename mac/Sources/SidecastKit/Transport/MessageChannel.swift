import Foundation
import Network
import Security

/// Framed message transport over an `NWConnection`. Every callback runs on
/// `queue`, and every method must be called on `queue`.
public final class MessageChannel {
    public let connection: NWConnection
    public let queue: DispatchQueue

    /// Connection is ready (TLS handshake done).
    public var onReady: (() -> Void)?
    /// A complete frame arrived (unknown types included; the caller skips them).
    public var onFrame: ((Frame) -> Void)?
    /// Called exactly once when the channel ends; nil error = orderly close.
    public var onClose: ((Error?) -> Void)?

    public private(set) var isClosed = false
    /// A final message was queued by `close(after:)`; further sends are dropped.
    public private(set) var isClosing = false
    /// Bytes handed to `send` whose transmission has not completed yet.
    public private(set) var pendingSendBytes = 0

    private let parser = FrameParser()
    private static let receiveChunk = 256 * 1024

    public init(connection: NWConnection, queue: DispatchQueue) {
        self.connection = connection
        self.queue = queue
    }

    public func start() {
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                self.onReady?()
                self.receiveNext()
            case .failed(let error):
                self.finish(error)
            case .waiting(let error):
                // A server-accepted or loopback connection that waits will not recover.
                self.finish(error)
            case .cancelled:
                self.finish(nil)
            default:
                break
            }
        }
        connection.start(queue: queue)
    }

    public func send(_ data: Data, completion: ((NWError?) -> Void)? = nil) {
        guard !isClosed, !isClosing else { return }
        enqueue(data, completion: completion)
    }

    private func enqueue(_ data: Data, completion: ((NWError?) -> Void)?) {
        pendingSendBytes += data.count
        connection.send(content: data, completion: .contentProcessed { [weak self] error in
            guard let self else { return }
            self.pendingSendBytes -= data.count
            if let error {
                self.finish(error)
            }
            completion?(error)
        })
    }

    /// Sends `last` (e.g. a BYE), then closes gracefully once it is out.
    public func close(after last: Data? = nil) {
        guard !isClosed, !isClosing else { return }
        isClosing = true
        guard let last else {
            connection.cancel()
            return
        }
        enqueue(last) { [weak self] _ in self?.connection.cancel() }
        // Do not wait forever on a peer that stopped reading.
        queue.asyncAfter(deadline: .now() + 2) { [weak self] in self?.connection.forceCancel() }
    }

    /// Immediate close.
    public func cancel() {
        connection.forceCancel()
        finish(nil)
    }

    /// SHA-256 of the peer's leaf certificate (client side: the server's cert).
    public func peerLeafFingerprint() -> Data? {
        guard let metadata = connection.metadata(definition: NWProtocolTLS.definition) as? NWProtocolTLS.Metadata
        else { return nil }
        var leaf: Data?
        sec_protocol_metadata_access_peer_certificate_chain(metadata.securityProtocolMetadata) { cert in
            guard leaf == nil else { return }
            let ref = sec_certificate_copy_ref(cert).takeRetainedValue()
            leaf = SecCertificateCopyData(ref) as Data
        }
        return leaf.map(CertificateFingerprint.sha256(der:))
    }

    // MARK: - Private

    private func receiveNext() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: Self.receiveChunk) {
            [weak self] content, _, isComplete, error in
            guard let self, !self.isClosed else { return }
            if let content, !content.isEmpty {
                self.parser.append(content)
                do {
                    while let frame = try self.parser.nextFrame() {
                        self.onFrame?(frame)
                        if self.isClosed { return }
                    }
                } catch {
                    self.connection.forceCancel()
                    self.finish(error)
                    return
                }
            }
            if let error {
                self.finish(error)
            } else if isComplete {
                self.connection.cancel()
                self.finish(nil)
            } else {
                self.receiveNext()
            }
        }
    }

    private func finish(_ error: Error?) {
        guard !isClosed else { return }
        isClosed = true
        let handler = onClose
        onClose = nil
        onFrame = nil
        onReady = nil
        handler?(error)
    }
}
