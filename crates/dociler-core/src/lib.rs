//! Shared Dociler services, independent of terminal rendering and HTTP transport.
//!
//! This first milestone provides filesystem diagnostics only. Document access,
//! configuration, inference, and permission grants will be added as bounded
//! services instead of being coupled to the CLI.

pub mod diagnostics;
