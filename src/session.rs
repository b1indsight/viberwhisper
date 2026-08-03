//! Shared session identity for routing work across recording subsystems.

/// Identifies one logical recording session across recorder and transcription components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);
