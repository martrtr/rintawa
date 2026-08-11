//! API available to Taverna extensions.
//!
//! This module provides APIs that are exposed to extensions at runtime,
//! such as logging and other system services.

mod logger;

pub use logger::{LogLevel, LoggerApi};
