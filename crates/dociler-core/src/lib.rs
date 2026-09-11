//! Shared Dociler services, independent of terminal rendering and HTTP transport.
//!
//! Configuration contains only preferences and approved workspace identities;
//! session content and credentials are deliberately not serializable settings.

pub mod config;
pub mod credentials;
pub mod diagnostics;
pub mod paths;
pub mod session;
pub mod workspace;
