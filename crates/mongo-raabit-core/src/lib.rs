//! Headless core of the mongo-raabit MongoDB client.
//!
//! No UI dependencies: the egui views and the CLI drive everything through this crate.
//! All network operations are `async` on Tokio; synchronous
//! hosts spawn them on their own Tokio runtime.

pub mod aggregate;
pub mod builder;
pub mod codegen;
pub mod connection;
pub mod connection_form;
pub mod documents;
pub mod error;
pub mod fields;
pub mod format;
pub mod indexes;
pub mod mongosh;
pub mod profile;
pub mod query;
pub mod schema;
pub mod secrets;
pub mod settings;
pub mod shell;
pub mod shell_eval;
pub mod sql;
pub mod sql_syntax;
pub mod stage_form;
pub mod status;
pub mod tabs;
pub mod transfer;
pub mod tunnel;
pub mod write;

pub use connection::{ConnectOptions, Connection};
pub use error::{RaabitError, Result};
pub use mongodb::{self, bson};
pub use settings::Settings;
pub use write::{Affected, Write, WriteOutcome, WritePreview};
