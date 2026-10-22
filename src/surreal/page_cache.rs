//! A Firecrawl response cache in SurrealDB that can also be searched by text.

use std::time::Duration;

use futures::future::BoxFuture;
use surrealdb::types::{Datetime, SurrealValue};
use surrealdb::{Connection, Surreal};
use time::OffsetDateTime;

use super::store_error;
use super::time::{from_datetime, to_datetime};
use crate::error::Result;
use crate::firecrawl::{FirecrawlCache, ScrapeResponse, SearchResponse};

const SCHEMA: &str = "
    DEFINE TABLE IF NOT EXISTS topcoat_page_cache TYPE NORMAL SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS key ON topcoat_page_cache TYPE string;
    DEFINE FIELD IF NOT EXISTS body ON topcoat_page_cache TYPE string;
    DEFINE FIELD IF NOT EXISTS url ON topcoat_page_cache TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS title ON topcoat_page_cache TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS text ON topcoat_page_cache TYPE string;
    DEFINE FIELD IF NOT EXISTS expires_at ON topcoat_page_cache TYPE datetime;
    DEFINE FIELD IF NOT EXISTS stored_at ON topcoat_page_cache TYPE datetime;
    DEFINE ANALYZER IF NOT EXISTS topcoat_english
        TOKENIZERS blank, class FILTERS lowercase, ascii, snowball(english);
    DEFINE INDEX IF NOT EXISTS topcoat_page_cache_text ON topcoat_page_cache
        FIELDS text FULLTEXT ANALYZER topcoat_english BM25;
";

// The record id is the key itself, as a string id. SurrealDB escapes any string as an id, so a
// long key made of JSON needs no hashing, and a repeat put replaces its own row.
const GET: &str = "
    SELECT VALUE body FROM type::record('topcoat_page_cache', $key) WHERE expires_at > time::now()
";
const PUT: &str = "
    UPSERT type::record('topcoat_page_cache', $key) CONTENT {
        key: $key,
        body: $body,
        url: $url,
        title: $title,
        text: $text,
        expires_at: $expires_at,
        stored_at: $stored_at,
    } RETURN NONE
";
const SEARCH: &str = "
    SELECT url, title, text, stored_at, search::score(1) AS score FROM topcoat_page_cache
    WHERE text @1@ $query AND expires_at > time::now()
    ORDER BY score DESC LIMIT $limit
";
// The count comes first, so no body is loaded just to be counted. Both statements use the same
// cutoff, so the count is the number of rows the delete removes.
const PURGE: &str = "
    SELECT VALUE count FROM (
        SELECT count() FROM topcoat_page_cache WHERE expires_at <= $now GROUP ALL
    );
    DELETE topcoat_page_cache WHERE expires_at <= $now RETURN NONE;
";

/// A [`FirecrawlCache`] kept in SurrealDB.
///
/// Each answer is stored with its page text, so pages already paid for can be found again by
/// [`PageCache::search`] without calling Firecrawl.
#[derive(Debug, Clone)]
pub struct PageCache<C: Connection> {
    db: Surreal<C>,
}

/// A cached page found by [`PageCache::search`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CachedPage {
    /// The page URL, when known.
    pub url: Option<String>,
    /// The page title, when known.
    pub title: Option<String>,
    /// The page markdown, or for a search answer its titles and descriptions.
    pub text: String,
    /// BM25 relevance to the query; higher is better.
    pub score: f64,
    /// When the answer was stored.
    pub stored_at: OffsetDateTime,
}

#[derive(SurrealValue)]
struct PageRow {
    url: Option<String>,
    title: Option<String>,
    text: String,
    stored_at: Datetime,
    score: f64,
}

/// What a body contributes to the searchable columns.
#[derive(Default)]
struct Extract {
    url: Option<String>,
    title: Option<String>,
    text: String,
}

impl<C: Connection> PageCache<C> {
    /// Defines the table and its text index if they are missing, then wraps `db`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the schema cannot be defined.
    pub async fn try_new(db: Surreal<C>) -> Result<Self> {
        db.query(SCHEMA)
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error("defining the page cache", error))?;
        Ok(Self { db })
    }

    /// The unexpired pages that best match `query`, best first. `limit` is clamped to 1..=50.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the query fails.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<CachedPage>> {
        let limit = limit.clamp(1, 50) as i64;
        let rows: Vec<PageRow> = self
            .db
            .query(SEARCH)
            .bind(("query", query.to_string()))
            .bind(("limit", limit))
            .await
            .and_then(|mut response| response.take(0))
            .map_err(|error| store_error("searching the page cache", error))?;
        Ok(rows
            .into_iter()
            .map(|row| CachedPage {
                url: row.url,
                title: row.title,
                text: row.text,
                score: row.score,
                stored_at: from_datetime(&row.stored_at),
            })
            .collect())
    }

    /// Deletes every expired row and returns how many there were.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the delete fails.
    pub async fn purge_expired(&self) -> Result<u64> {
        let removed: Vec<u64> = self
            .db
            .query(PURGE)
            .bind(("now", to_datetime(OffsetDateTime::now_utc())))
            .await
            .and_then(|mut response| response.take(0))
            .map_err(|error| store_error("purging the page cache", error))?;
        Ok(removed.into_iter().sum())
    }

    async fn lookup(&self, key: &str) -> Result<Option<String>> {
        let bodies: Vec<String> = self
            .db
            .query(GET)
            .bind(("key", key.to_string()))
            .await
            .and_then(|mut response| response.take(0))
            .map_err(|error| store_error("reading the page cache", error))?;
        Ok(bodies.into_iter().next())
    }

    async fn store(&self, key: &str, body: String, ttl: Duration) -> Result<()> {
        let extract = extract(key, &body);
        let now = OffsetDateTime::now_utc();
        // A ttl too long to represent never expires.
        let expires_at = now
            .checked_add(ttl.try_into().unwrap_or(time::Duration::MAX))
            .unwrap_or(time::PrimitiveDateTime::MAX.assume_utc());
        self.db
            .query(PUT)
            .bind(("key", key.to_string()))
            .bind(("body", body))
            .bind(("url", extract.url))
            .bind(("title", extract.title))
            .bind(("text", extract.text))
            .bind(("expires_at", to_datetime(expires_at)))
            .bind(("stored_at", to_datetime(now)))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error("writing the page cache", error))?;
        Ok(())
    }
}

impl<C: Connection> FirecrawlCache for PageCache<C> {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            match self.lookup(key).await {
                Ok(body) => body,
                Err(error) => {
                    tracing::warn!(
                        target: "topcoat_si::firecrawl",
                        error = &error as &dyn std::error::Error,
                        "Page cache lookup failed; treating it as a miss"
                    );
                    None
                }
            }
        })
    }

    fn put<'a>(&'a self, key: &'a str, body: String, ttl: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Err(error) = self.store(key, body, ttl).await {
                tracing::warn!(
                    target: "topcoat_si::firecrawl",
                    error = &error as &dyn std::error::Error,
                    "Page cache store failed; dropping the answer"
                );
            }
        })
    }
}

/// Reads the searchable columns out of a cached `body`; a body that does not parse gives empty
/// ones.
fn extract(key: &str, body: &str) -> Extract {
    if let Some(request) = key.strip_prefix("scrape:") {
        let Ok(response) = serde_json::from_str::<ScrapeResponse>(body) else {
            return Extract::default();
        };
        let metadata = response.data.metadata;
        let url = metadata
            .as_ref()
            .and_then(|metadata| metadata.source_url.clone())
            .or_else(|| request_url(request));
        Extract {
            url,
            title: metadata.and_then(|metadata| metadata.title),
            text: response.data.markdown.unwrap_or_default(),
        }
    } else if key.starts_with("search:") {
        let Ok(response) = serde_json::from_str::<SearchResponse>(body) else {
            return Extract::default();
        };
        let lines: Vec<String> = response
            .data
            .and_then(|data| data.web)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|result| [result.title, result.description])
            .flatten()
            .collect();
        Extract {
            text: lines.join("\n"),
            ..Extract::default()
        }
    } else {
        Extract::default()
    }
}

/// The `url` of a request body.
fn request_url(request: &str) -> Option<String> {
    let request: serde_json::Value = serde_json::from_str(request).ok()?;
    request.get("url")?.as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surreal::test_db::temp_db;

    const HOUR: Duration = Duration::from_secs(3600);

    fn scrape_key(url: &str) -> String {
        format!("scrape:{}", serde_json::json!({ "url": url }))
    }

    fn scrape_body(markdown: &str) -> String {
        serde_json::json!({
            "success": true,
            "data": {"markdown": markdown, "metadata": {"title": "A page"}}
        })
        .to_string()
    }

    #[tokio::test]
    async fn put_then_get_returns_the_body() {
        let cache = PageCache::try_new(temp_db().await).await.unwrap();
        let key = scrape_key("https://example.com/a");
        cache.put(&key, scrape_body("hello"), HOUR).await;
        assert_eq!(cache.get(&key).await, Some(scrape_body("hello")));
        assert_eq!(cache.get("scrape:other").await, None);
    }

    #[tokio::test]
    async fn an_enormous_ttl_never_expires() {
        let cache = PageCache::try_new(temp_db().await).await.unwrap();
        let key = scrape_key("https://example.com/a");
        cache.put(&key, scrape_body("hello"), Duration::MAX).await;
        assert_eq!(cache.get(&key).await, Some(scrape_body("hello")));
    }

    #[tokio::test]
    async fn a_zero_ttl_is_not_returned() {
        let cache = PageCache::try_new(temp_db().await).await.unwrap();
        let key = scrape_key("https://example.com/a");
        cache.put(&key, scrape_body("hello"), Duration::ZERO).await;
        assert_eq!(cache.get(&key).await, None);
    }

    #[tokio::test]
    async fn search_finds_the_matching_page() {
        let cache = PageCache::try_new(temp_db().await).await.unwrap();
        let boats = scrape_key("https://example.com/boats");
        let rust = scrape_key("https://example.com/rust");
        cache
            .put(&boats, scrape_body("quarterly boat storage rates"), HOUR)
            .await;
        cache.put(&rust, scrape_body("rust editions"), HOUR).await;
        let found = cache.search("boat storage", 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].url.as_deref(), Some("https://example.com/boats"));
        assert_eq!(found[0].title.as_deref(), Some("A page"));
        assert_eq!(found[0].text, "quarterly boat storage rates");
        assert!(found[0].score.is_finite());
    }

    #[tokio::test]
    async fn purge_expired_removes_the_expired_row() {
        let cache = PageCache::try_new(temp_db().await).await.unwrap();
        cache
            .put(
                &scrape_key("https://example.com/old"),
                scrape_body("old"),
                Duration::ZERO,
            )
            .await;
        cache
            .put(
                &scrape_key("https://example.com/new"),
                scrape_body("new"),
                HOUR,
            )
            .await;
        assert_eq!(cache.purge_expired().await.unwrap(), 1);
        assert_eq!(cache.purge_expired().await.unwrap(), 0);
    }
}
