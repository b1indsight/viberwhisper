//! Desktop listener and setup interaction using the existing core and platform services.

pub(crate) mod listener;
pub(crate) mod setup;

#[cfg(feature = "integration")]
pub(crate) mod preview_demo;
