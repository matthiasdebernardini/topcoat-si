//! Semantic and keyword search over an app's own documents through turbopuffer.
//!
//! [`TurbopufferClient`] talks to turbopuffer's v2 API. [`TurbopufferClient::namespace`] picks a
//! namespace and the embedding model turbopuffer embeds each document's text with, so the app
//! never calls an embedding provider itself. A [`Namespace`] writes [`Document`]s, deletes them
//! and runs a [`Query`]: hybrid (the default), vector or keyword, narrowed by a [`Filter`].
//! [`TurbopufferSearch`] exposes one namespace to Claude as a read-only client-side search tool
//! with citable results.
//!
//! The text is sent to the model's provider for embedding, which retains none of it. The model
//! is fixed for the life of a namespace: changing it means re-ingesting every document.
//!
//! ```no_run
//! # async fn run() -> topcoat_si::Result<()> {
//! use topcoat_si::turbopuffer::{Document, Filter, Query, SearchMode, TurbopufferClient};
//!
//! let client = TurbopufferClient::from_env()?;
//! let help = client.namespace("acme-help", "voyage/voyage-4-lite")?;
//! help.upsert([Document::new("refunds", "Refunds go back to the original card.")
//!     .with_attribute("kind", "policy")])
//!     .await?;
//! let found = help
//!     .query(
//!         &Query::new("can I get my money back?")
//!             .with_mode(SearchMode::Hybrid)
//!             .with_filter(Filter::equals("kind", "policy")),
//!     )
//!     .await?;
//! for hit in &found.hits {
//!     println!("{} {:.3}", hit.id, hit.score);
//! }
//! # Ok(())
//! # }
//! ```

mod client;
mod document;
mod filter;
mod namespace;
mod query;
mod search;

#[cfg(test)]
mod tests;

pub use client::TurbopufferClient;
pub use document::{Document, DocumentId};
pub use filter::Filter;
pub use namespace::{Namespace, UpsertSummary};
pub use query::{Hit, Query, QueryResponse, SearchMode};
pub use search::TurbopufferSearch;
