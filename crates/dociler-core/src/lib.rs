//! Shared Dociler services, independent of terminal rendering and HTTP transport.
//!
//! Configuration contains only preferences and approved workspace identities;
//! session content and credentials are deliberately not serializable settings.

pub mod assets;
pub mod cancellation;
pub mod chat;
pub mod config;
pub mod credentials;
pub mod diagnostics;
pub mod downloads;
pub mod hardware;
pub mod model_probe;
pub mod paths;
pub mod profiles;
pub mod remote;
pub mod runtime_install;
pub mod runtime_probe;
pub mod session;
pub mod workspace;
