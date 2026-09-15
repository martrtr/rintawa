//! Local extension development tools.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod config;
mod error;
mod project;
mod reload;
mod session;
mod web;

pub use config::{DEV_CONFIG_FILE, DevConfig};
pub use error::{DevError, DevResult};
pub use project::{DevProject, PreparedSnapshot, SourceRevision};
pub use reload::{ReloadOutcome, ReloadingDevSession};
pub use session::DevSession;
