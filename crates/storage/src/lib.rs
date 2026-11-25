//! Persistent storage implementations for authoritative Rintawa worlds.
//!
//! SQLite is an implementation detail of this crate. Extensions do not receive
//! database handles and interact with world state through higher-level runtime contracts.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod error;
mod sqlite;

pub use error::{StorageError, StorageResult};
pub use sqlite::{SqliteWorldSnapshot, SqliteWorldStorage};
