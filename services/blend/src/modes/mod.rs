mod broadcast;
mod core;
mod edge;
mod ondemand;

use std::fmt::Debug;

use lb_log_targets::blend;

#[cfg(test)]
pub use crate::modes::broadcast::tests as broadcast_tests;
pub use crate::modes::{broadcast::BroadcastMode, core::CoreMode, edge::EdgeMode};

const LOG_TARGET: &str = blend::service::MODES;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Overwatch error: {0}")]
    Overwatch(#[from] overwatch::overwatch::Error),
    #[error("Service error: {0}")]
    Service(#[from] overwatch::DynError),
    #[error("Relay send error: {0}")]
    RelaySend(String),
}
