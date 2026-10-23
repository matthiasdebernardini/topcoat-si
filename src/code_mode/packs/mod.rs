//! The built-in packs.
//!
//! Packs holding host functions keep per-conversation state (the Firecrawl credit budget, the
//! Jev budget, concurrency limit and observer), so build them for each
//! [`CodeMode`](super::CodeMode).

mod firecrawl;
mod jev;
mod reports;
mod research;
#[cfg(feature = "surreal")]
mod surreal;
mod triage;

pub use firecrawl::firecrawl;
pub use jev::{JevBudget, JevEvent, JevObserver, JevPack, JevUsed, jev};
pub use reports::reports;
pub use research::research;
#[cfg(feature = "surreal")]
pub use surreal::surreal;
pub use triage::triage;
