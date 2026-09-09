//! Rintawa SDK — public contracts for developing extensions.
//!
//! Version 0.0.1 intentionally provides a small lifecycle and manifest
//! contract. It does not implement the extension engine, runtime hosts, state,
//! storage, AI, or UI rendering. It defines declarative secret-read requests;
//! the Rintawa host alone evaluates and grants them.
//!
//! A runtime component uses `kind = "runtime"` in its manifest. Its execution
//! model is selected by its target: `"native"` for the initial host and
//! `"wasm"` when a WIT-based adapter is introduced. React is an implementation
//! detail of the official Web UI Host; it is not a dependency or public type of
//! this crate.
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
pub mod contributions;
pub mod errors;
pub mod manifest;
pub mod runtime_effects;
pub mod secrets;
pub mod traits;
pub mod types;

pub mod prelude;
