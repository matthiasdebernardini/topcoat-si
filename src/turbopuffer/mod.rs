//! Claude's read-only search over an app's own documents in turbopuffer.
//!
//! Writing and querying go through turbopuffer's official SDK, the `turbopuffer` crate, which is
//! generated from turbopuffer's API spec and so stays current. [`Client`], [`Namespace`] and
//! [`Filter`] are re-exported from it; depend on `turbopuffer` itself, at the same version, for
//! the rest of its API. [`TurbopufferSearch`] exposes one namespace to Claude as a client-side
//! search tool with citable results: hybrid (the default), vector or keyword.
//!
//! The tool searches the `text` attribute. Write it with an embedding model in the schema, such
//! as `AttributeSchemaConfig::new("string").embed("voyage/voyage-4-lite").full_text_search(true)`,
//! so turbopuffer embeds the text itself and queries embed with the same model. The text is sent
//! to the model's provider for embedding, which retains none of it. The model is fixed for the
//! life of a namespace: changing it means re-ingesting every document.
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use carpa::turbopuffer::{Client, Filter, TurbopufferSearch};
//! use turbopuffer::types::{AttributeSchemaConfig, Row};
//!
//! let client = Client::from_env()?;
//! let help = client.namespace("acme-help");
//! help.write()
//!     .upsert_rows([Row::new("refunds")
//!         .insert("text", "Refunds go back to the original card.")
//!         .insert("kind", "policy")])
//!     .schema([(
//!         "text",
//!         AttributeSchemaConfig::new("string")
//!             .embed("voyage/voyage-4-lite")
//!             .full_text_search(true),
//!     )])
//!     .distance_metric("cosine_distance")
//!     .send()
//!     .await?;
//! let search = TurbopufferSearch::try_new(help, "search_help", "Acme's help articles")?
//!     .with_filter(Filter::eq("kind", "policy"))?;
//! # let _ = search;
//! # Ok(())
//! # }
//! ```

mod search;

#[cfg(test)]
mod tests;

#[cfg(feature = "code-mode")]
pub(crate) use search::SearchMode;
pub use search::TurbopufferSearch;
pub use turbopuffer::types::Filter;
pub use turbopuffer::{Client, Namespace};
