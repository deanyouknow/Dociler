//! Shared Dociler services, independent of terminal rendering and HTTP transport.
//!
//! Configuration contains only preferences and approved workspace identities;
//! session content and credentials are deliberately not serializable settings.

pub mod assets;
pub mod cancellation;
pub mod chat;
pub mod config;
pub mod context;
pub mod credentials;
pub mod diagnostics;
pub mod document;
pub mod downloads;
pub mod editing;
pub mod evaluation;
pub mod export;
pub mod extractor;
pub mod hardware;
pub mod indexing;
pub mod model_probe;
pub mod paths;
mod probe_memory;
pub mod profiles;
pub mod remote;
pub mod remote_serve;
pub mod runtime_install;
pub mod runtime_probe;
pub mod runtime_router;
pub mod session;
pub mod skills;
pub mod workspace;
