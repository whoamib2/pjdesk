//! Android (and any other mobile) session engine.
//!
//! The UI is Kotlin. This library speaks the same signaling, relay, and
//! peer protocol as the Windows client, and hands H.264 access units and
//! input across the boundary.

mod engine;

#[cfg(target_os = "android")]
mod jni;

pub use engine::{permanent_ok, Callbacks, Engine, APP_VERSION};
pub use pjdesk_proto::gesture::{self, letterbox, view_to_norm, Frame, Letterbox, StrokeTracker, View};
