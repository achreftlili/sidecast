//! Windows backends: not implemented yet (ROADMAP P2, a separate pull request).
//!
//! The pull request that implements them provides here:
//! - `DisplayBackend`: add a virtual monitor through the signed IddCx
//!   Virtual Display Driver at `plan.display` (or use a chosen monitor),
//!   capture it with Windows.Graphics.Capture, hand frames to the encoder
//!   (BGRA/NV12 CPU buffers, or D3D11 textures as `PixelData::Native`);
//! - `EncoderFactory`s: NVENC / AMF / Quick Sync / Media Foundation, with
//!   OpenH264 (`OpenH264Factory`) as the last fallback;
//! - `InputBackend`: SendInput (mouse, keys, Unicode text), mapping
//!   `SemanticAction`s to Win+Ctrl+←/→ (desktops) and Win+Tab (Task View);
//! - `PowerGuard`: SetThreadExecutionState(ES_SYSTEM_REQUIRED | ES_CONTINUOUS);
//! - `MicSink`: a virtual audio cable fed from the jitter buffer.

use sidecast_server::backend::{BackendError, Backends};

pub fn backends() -> Result<Backends, BackendError> {
    Err(BackendError::not_supported("the Windows host is not supported yet (planned: ROADMAP P2); try --test-pattern"))
}
