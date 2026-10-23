//! The built-in packs.
//!
//! Packs holding host functions keep per-conversation state (the Firecrawl and Jev budgets, the
//! Jev concurrency limit and the observers), so build them for each
//! [`CodeMode`](super::CodeMode).

mod firecrawl;
mod jev;
mod reports;
mod research;
#[cfg(feature = "surreal")]
mod surreal;
mod triage;
#[cfg(feature = "turbopuffer")]
mod turbopuffer;

pub use firecrawl::{
    FirecrawlBudget, FirecrawlEvent, FirecrawlHit, FirecrawlObserver, FirecrawlPack, FirecrawlPage,
    FirecrawlUsed, OutOfCredits, firecrawl,
};
pub use jev::{JevBudget, JevEvent, JevObserver, JevPack, JevUsed, jev};
pub use reports::reports;
pub use research::research;
#[cfg(feature = "surreal")]
pub use surreal::surreal;
pub use triage::triage;
#[cfg(feature = "turbopuffer")]
pub use turbopuffer::turbopuffer;
