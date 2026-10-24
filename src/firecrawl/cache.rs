//! The hook that lets a [`FirecrawlClient`](super::FirecrawlClient) reuse answers it has paid for.

use std::time::Duration;

use futures::future::BoxFuture;

/// Where `FirecrawlClient` keeps answers it has already paid for. Keys are opaque strings.
///
/// The cache never fails a request: an implementation treats a lookup that errors as a miss and
/// drops a store that errors, logging it at `tracing::warn!` with target `carpa::firecrawl`.
pub trait FirecrawlCache: Send + Sync {
    /// The cached body for `key`, if it is still fresh.
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<String>>;
    /// Stores `body` under `key` for `ttl`.
    fn put<'a>(&'a self, key: &'a str, body: String, ttl: Duration) -> BoxFuture<'a, ()>;
}
