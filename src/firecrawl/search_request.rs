//! Search parameters.

use std::time::Duration;

use serde::Serialize;
use time::Date;

use super::scrape_options::{ScrapeOptions, WireScrapeOptions, millis};
use crate::error::{Error, Result};

const MAX_QUERY_CHARS: usize = 500;

/// Where Firecrawl searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SearchSource {
    /// Regular web results.
    Web,
    /// News articles.
    News,
    /// Images.
    Images,
}

/// Restricts results to a kind of site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SearchCategory {
    /// Code hosts and developer documentation. Cannot be combined with other categories.
    Developer,
    /// Research papers and academic sites.
    Research,
    /// PDF documents.
    Pdf,
    /// Government sites. Cannot be combined with other categories.
    Gov,
}

/// Limits results to a publication window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TimeRange {
    /// The past hour.
    PastHour,
    /// The past day.
    PastDay,
    /// The past week.
    PastWeek,
    /// The past month.
    PastMonth,
    /// The past year.
    PastYear,
    /// An inclusive date range.
    Custom {
        /// First day of the range.
        from: Date,
        /// Last day of the range.
        to: Date,
    },
}

impl TimeRange {
    fn tbs(&self) -> String {
        let us_date = |date: &Date| {
            format!(
                "{:02}/{:02}/{:04}",
                u8::from(date.month()),
                date.day(),
                date.year()
            )
        };
        match self {
            Self::PastHour => "qdr:h".to_string(),
            Self::PastDay => "qdr:d".to_string(),
            Self::PastWeek => "qdr:w".to_string(),
            Self::PastMonth => "qdr:m".to_string(),
            Self::PastYear => "qdr:y".to_string(),
            Self::Custom { from, to } => {
                format!("cdr:1,cd_min:{},cd_max:{}", us_date(from), us_date(to))
            }
        }
    }
}

/// Parameters for [`FirecrawlClient::search`](super::FirecrawlClient::search).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchRequest {
    /// The search query, at most 500 characters.
    pub query: String,
    /// Results per source, 1 to 100. Firecrawl defaults to 10.
    pub limit: Option<u32>,
    /// Sources to search. Empty means Firecrawl's default (web).
    pub sources: Vec<SearchSource>,
    /// Categories to restrict results to.
    pub categories: Vec<SearchCategory>,
    /// Only return results from these hostnames. Exclusive with `exclude_domains`.
    pub include_domains: Vec<String>,
    /// Never return results from these hostnames. Exclusive with `include_domains`.
    pub exclude_domains: Vec<String>,
    /// Publication window.
    pub time_range: Option<TimeRange>,
    /// Order results by date instead of relevance.
    pub sort_by_date: bool,
    /// Location to search from, such as `"Germany"` or `"San Francisco,California,United States"`.
    pub location: Option<String>,
    /// ISO country code to search from, such as `"US"`.
    pub country: Option<String>,
    /// Server-side time limit for the whole search.
    pub timeout: Option<Duration>,
    /// Drop invalid URLs from results instead of failing.
    pub ignore_invalid_urls: Option<bool>,
    /// Put the passage that best answers the query in each web result's `description` and each
    /// news result's `snippet`. Firecrawl defaults to true; false returns the sites' own
    /// descriptions and snippets.
    pub highlights: Option<bool>,
    /// Scrape each result's page.
    pub scrape_options: Option<ScrapeOptions>,
}

impl SearchRequest {
    /// Create a request for `query` with Firecrawl's defaults.
    #[must_use]
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: None,
            sources: Vec::new(),
            categories: Vec::new(),
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            time_range: None,
            sort_by_date: false,
            location: None,
            country: None,
            timeout: None,
            ignore_invalid_urls: None,
            highlights: None,
            scrape_options: None,
        }
    }

    /// Set the number of results per source.
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the sources to search.
    #[must_use]
    pub fn with_sources(mut self, sources: Vec<SearchSource>) -> Self {
        self.sources = sources;
        self
    }

    /// Set the categories to restrict results to.
    #[must_use]
    pub fn with_categories(mut self, categories: Vec<SearchCategory>) -> Self {
        self.categories = categories;
        self
    }

    /// Only return results from these hostnames.
    #[must_use]
    pub fn with_include_domains(mut self, domains: Vec<String>) -> Self {
        self.include_domains = domains;
        self
    }

    /// Never return results from these hostnames.
    #[must_use]
    pub fn with_exclude_domains(mut self, domains: Vec<String>) -> Self {
        self.exclude_domains = domains;
        self
    }

    /// Set the publication window.
    #[must_use]
    pub fn with_time_range(mut self, time_range: TimeRange) -> Self {
        self.time_range = Some(time_range);
        self
    }

    /// Order results by date instead of relevance.
    #[must_use]
    pub fn with_sort_by_date(mut self, sort_by_date: bool) -> Self {
        self.sort_by_date = sort_by_date;
        self
    }

    /// Set the location to search from.
    #[must_use]
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Set the ISO country code to search from.
    #[must_use]
    pub fn with_country(mut self, country: impl Into<String>) -> Self {
        self.country = Some(country.into());
        self
    }

    /// Set the server-side time limit.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Drop invalid URLs from results instead of failing.
    #[must_use]
    pub fn with_ignore_invalid_urls(mut self, ignore: bool) -> Self {
        self.ignore_invalid_urls = Some(ignore);
        self
    }

    /// Set whether descriptions and snippets are the passages that best answer the query.
    #[must_use]
    pub fn with_highlights(mut self, highlights: bool) -> Self {
        self.highlights = Some(highlights);
        self
    }

    /// Scrape each result's page.
    #[must_use]
    pub fn with_scrape_options(mut self, scrape_options: ScrapeOptions) -> Self {
        self.scrape_options = Some(scrape_options);
        self
    }

    /// Check the constraints Firecrawl would otherwise reject with a 400.
    pub fn validate(&self) -> Result<()> {
        let invalid =
            |message: &str, param: &str| Err(Error::validation(message, Some(param.to_string())));
        if self.query.trim().is_empty() {
            return invalid("query must not be empty", "query");
        }
        if self.query.chars().count() > MAX_QUERY_CHARS {
            return invalid("query must be at most 500 characters", "query");
        }
        if self.limit.is_some_and(|limit| !(1..=100).contains(&limit)) {
            return invalid("limit must be between 1 and 100", "limit");
        }
        if !self.include_domains.is_empty() && !self.exclude_domains.is_empty() {
            return invalid(
                "include_domains and exclude_domains are mutually exclusive",
                "include_domains",
            );
        }
        let exclusive = self
            .categories
            .iter()
            .any(|c| matches!(c, SearchCategory::Gov | SearchCategory::Developer));
        if exclusive && self.categories.len() > 1 {
            return invalid(
                "the gov and developer categories cannot be combined with other categories",
                "categories",
            );
        }
        if let Some(TimeRange::Custom { from, to }) = self.time_range
            && from > to
        {
            return invalid("custom time range starts after it ends", "time_range");
        }
        if let Some(scrape_options) = &self.scrape_options {
            scrape_options.validate()?;
        }
        Ok(())
    }

    pub(super) fn wire(&self) -> WireSearchRequest<'_> {
        let tbs: Vec<String> = self
            .time_range
            .iter()
            .map(TimeRange::tbs)
            .chain(self.sort_by_date.then(|| "sbd:1".to_string()))
            .collect();
        WireSearchRequest {
            query: &self.query,
            limit: self.limit,
            sources: &self.sources,
            categories: &self.categories,
            include_domains: &self.include_domains,
            exclude_domains: &self.exclude_domains,
            tbs: (!tbs.is_empty()).then(|| tbs.join(",")),
            location: self.location.as_deref(),
            country: self.country.as_deref(),
            timeout: self.timeout.map(millis),
            ignore_invalid_urls: self.ignore_invalid_urls,
            highlights: self.highlights,
            scrape_options: self.scrape_options.as_ref().map(ScrapeOptions::wire),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireSearchRequest<'request> {
    query: &'request str,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    sources: &'request [SearchSource],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    categories: &'request [SearchCategory],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    include_domains: &'request [String],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    exclude_domains: &'request [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    tbs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'request str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<&'request str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
    #[serde(rename = "ignoreInvalidURLs", skip_serializing_if = "Option::is_none")]
    ignore_invalid_urls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    highlights: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scrape_options: Option<WireScrapeOptions>,
}
