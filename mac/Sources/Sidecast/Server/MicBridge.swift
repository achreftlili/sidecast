import Foundation
import SidecastKit

/// The Mac side of the phone microphone (PROTOCOL §7.2) for the streaming
/// session. The real implementation (`MicRouter`) drives Core Audio; test mode
/// uses `TestMicBridge`. All calls arrive on the server queue; implementations
/// hop to their own queue.
protocol MicBridge: AnyObject {
    /// A session started streaming (first STREAM_CONFIG). `control(active)`
    /// sends MIC_CONTROL to it (any thread); the mic starts off.
    func sessionStarted(id: UUID, deviceName: String, supportsMic: Bool, control: @escaping (Bool) -> Void)

    /// The session stopped streaming (closed, replaced). Unknown ids are ignored.
    func sessionEnded(id: UUID)

    /// AUDIO while the last MIC_CONTROL sent to that session was active:true.
    func handleAudio(_ packet: AudioPacket, from id: UUID)

    /// MIC_STATE while the last MIC_CONTROL sent to that session was active:true.
    func handleMicState(_ state: MicState, from id: UUID)
}
