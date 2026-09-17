//! Rintawa SDK — public contracts for developing extensions.
//!
//! Version 0.0.1 intentionally provides a small lifecycle and manifest
//! contract. It does not implement the extension engine, runtime hosts, state,
//! storage, AI, or UI rendering. It defines declarative secret-read requests;
//! the Rintawa host alone evaluates and grants them.
//!
//! A component's execution model is selected by its target. Core permanently
//! provides the `rintawa.runtime.wasm-component@1` root target; additional targets
//! are expected to come from extensions rather than feature-specific Core crates.
//! Product roles are expressed through versioned contracts such as
//! `rintawa.host.shell@1`, independently from execution target or presentation
//! technology.
//!
//! ## Modules
//!
//! - [`types`] — identifiers and primitive types
//! - [`contributions`] — contribution descriptions
//! - [`manifest`] — manifest file structure
//! - [`traits`] — traits for component implementation
//! - [`context`] — execution context
//! - [`api`] — APIs available to extensions
//! - [`errors`] — error types
//! - [`secrets`] — validated secret paths and redacted secret values
//! - [`prelude`] — commonly used imports

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

pub mod api;
pub mod context;
pub mod contracts;
pub mod contributions;
pub mod errors;
pub mod manifest;
pub mod runtime_effects;
pub mod secrets;
pub mod services;
pub mod traits;
pub mod types;
pub mod ui;

pub mod prelude;
