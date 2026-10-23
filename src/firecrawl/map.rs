//! Site map: parameters and response.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const MAX_MAP_LIMIT: u32 = 100_000;

/// Whether map reads the site's sitemap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SitemapMode {
    /// Ignore the sitemap and only follow links.
    Skip,
    /// Use the sitemap and links. Firecrawl's default.
    Include,
    /// Use only the sitemap.
    Only,
}

/// Where map fetches the site from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct MapLocation {
    /// Two-letter uppercase ISO country code, such as `"US"`.
    pub country: String,
    /// Preferred languages in priority order, such as `"en-US"`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
}

impl MapLocation {
    /// Fetch from `country` with no language preference.
    #[must_use]
    pub fn new(country: impl Into<String>) -> Self {
        Self {
            country: country.into(),
            languages: Vec::new(),
        }
    }

    /// Set the preferred languages.
    #[must_use]
    pub fn with_languages(mut self, languages: Vec<String>) -> Self {
        self.languages = languages;
        self
    }
}

/// Parameters for [`FirecrawlClient::map`](super::FirecrawlClient::map).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MapRequest {
    /// Absolute http or https URL of the site.
    pub url: String,
    /// Order links by relevance to this query, such as `"planning commission agenda"`.
    pub search: Option<String>,
    /// Most links to return, 1 to 100,000. Firecrawl defaults to 5,000.
    pub limit: Option<u32>,
    /// Whether to read the sitemap. Firecrawl defaults to [`SitemapMode::Include`].
    pub sitemap: Option<SitemapMode>,
    /// Where to fetch the site from. Firecrawl defaults to the US.
    pub location: Option<MapLocation>,
}

impl MapRequest {
    /// Create a request for `url` with Firecrawl's defaults.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            search: None,
            limit: None,
            sitemap: None,
            location: None,
        }
    }

    /// Order links by relevance to `search`.
    #[must_use]
    pub fn with_search(mut self, search: impl Into<String>) -> Self {
        self.search = Some(search.into());
        self
    }

    /// Set the most links to return.
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set whether to read the sitemap.
    #[must_use]
    pub fn with_sitemap(mut self, sitemap: SitemapMode) -> Self {
        self.sitemap = Some(sitemap);
        self
    }

    /// Set where to fetch the site from.
    #[must_use]
    pub fn with_location(mut self, location: MapLocation) -> Self {
        self.location = Some(location);
        self
    }

    /// Check the constraints Firecrawl would otherwise reject with a 400.
    pub fn validate(&self) -> Result<()> {
        let invalid =
            |message: &str, param: &str| Err(Error::validation(message, Some(param.to_string())));
        let absolute = url::Url::parse(self.url.trim())
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
        if absolute.is_none() {
            return invalid("url must be an absolute http or https URL", "url");
        }
        if self
            .limit
            .is_some_and(|limit| !(1..=MAX_MAP_LIMIT).contains(&limit))
        {
            return invalid("limit must be between 1 and 100000", "limit");
        }
        if let Some(location) = &self.location
            && !(location.country.len() == 2
                && location.country.bytes().all(|b| b.is_ascii_uppercase()))
        {
            return invalid(
                "location country must be a two-letter uppercase code",
                "location",
            );
        }
        Ok(())
    }

    pub(super) fn wire(&self) -> WireMapRequest<'_> {
        WireMapRequest {
            url: self.url.trim(),
            search: self.search.as_deref(),
            limit: self.limit,
            sitemap: self.sitemap,
            location: self.location.as_ref(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct WireMapRequest<'request> {
    url: &'request str,
    #[serde(skip_serializing_if = "Option::is_none")]
    search: Option<&'request str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sitemap: Option<SitemapMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'request MapLocation>,
}

/// A successful site map.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct MapResponse {
    /// Whether the map succeeded.
    pub success: bool,
    /// The links found, most relevant first when a search was given.
    #[serde(default)]
    pub links: Vec<MapLink>,
    /// Credits the map cost, read from `creditsUsed` if Firecrawl sends it. A map served from
    /// [`FirecrawlClient::with_cache`](super::FirecrawlClient::with_cache) always reports 0.
    /// Never written back out, so a cached map does not keep it.
    #[serde(default, rename = "creditsUsed", skip_serializing)]
    pub credits_used: Option<u64>,
}

/// A link found by map.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct MapLink {
    /// Page URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Page title, when Firecrawl knows it.
    #[serde(default)]
    pub title: Option<String>,
    /// Page description, when Firecrawl knows it.
    #[serde(default)]
    pub description: Option<String>,
}
