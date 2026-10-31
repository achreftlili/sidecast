//! Stream negotiation and flow control (PROTOCOL §5, §6).

mod flow;
mod planner;

pub use flow::{AbrConfig, AbrDecision, AdaptiveBitrateController, InFlightTracker};
pub use planner::{
    BitrateBounds, EncoderCapability, StreamPlan, StreamPlanError, StreamPlanner, StreamSettings, VideoCodec,
    VirtualDisplayMode,
};
