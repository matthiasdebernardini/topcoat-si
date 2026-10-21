//! The built-in packs.
//!
//! Packs holding host functions keep per-conversation state (the Firecrawl credit budget), so
//! build them for each [`CodeMode`](super::CodeMode).

mod firecrawl;
mod jev;
mod reports;
mod research;
mod triage;

pub use firecrawl::firecrawl;
pub use jev::jev;
pub use reports::reports;
pub use research::research;
pub use triage::triage;
