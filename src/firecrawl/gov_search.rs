//! Government Index search: parameters and response.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The kind of court an opinion came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum CourtType {
    /// The US Supreme Court.
    FederalSupreme,
    /// Federal courts of appeals.
    FederalAppellate,
    /// Federal district courts.
    FederalDistrict,
    /// Federal bankruptcy courts.
    FederalBankruptcy,
    /// Other federal courts, such as the Tax Court.
    FederalSpecial,
    /// Military courts.
    Military,
    /// State supreme courts.
    StateSupreme,
    /// State appellate courts.
    StateAppellate,
    /// State trial courts.
    StateTrial,
    /// Other state courts.
    StateSpecial,
    /// State attorney general opinions.
    StateAttorneyGeneral,
}

/// Parameters for [`FirecrawlClient::gov_search`](super::FirecrawlClient::gov_search).
///
/// Filters combine with AND and only match pages Firecrawl has labeled.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GovSearchRequest {
    /// The search query.
    pub query: String,
    /// Number of results, 1 to 100. Firecrawl defaults to 10.
    pub k: Option<u32>,
    /// `"us"` for federal, or `"us-"` and a two-letter state or territory code, such as
    /// `"us-tn"`. Sent in lowercase, so `"US-TN"` works too. There is no city or county filter.
    pub jurisdiction: Option<String>,
    /// CourtListener court id, such as `"scotus"` or `"ca9"`.
    pub court: Option<String>,
    /// Kind of court.
    pub court_type: Option<CourtType>,
    /// Only documents citing this citation.
    pub cites: Option<String>,
    /// Only documents from this domain, such as `"irs.gov"`.
    pub domain: Option<String>,
}

impl GovSearchRequest {
    /// Create a request for `query` with Firecrawl's defaults and no filters.
    #[must_use]
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            k: None,
            jurisdiction: None,
            court: None,
            court_type: None,
            cites: None,
            domain: None,
        }
    }

    /// Set the number of results.
    #[must_use]
    pub fn with_k(mut self, k: u32) -> Self {
        self.k = Some(k);
        self
    }

    /// Only return documents from this jurisdiction.
    #[must_use]
    pub fn with_jurisdiction(mut self, jurisdiction: impl Into<String>) -> Self {
        self.jurisdiction = Some(jurisdiction.into());
        self
    }

    /// Only return opinions from this court.
    #[must_use]
    pub fn with_court(mut self, court: impl Into<String>) -> Self {
        self.court = Some(court.into());
        self
    }

    /// Only return opinions from this kind of court.
    #[must_use]
    pub fn with_court_type(mut self, court_type: CourtType) -> Self {
        self.court_type = Some(court_type);
        self
    }

    /// Only return documents citing this citation.
    #[must_use]
    pub fn with_cites(mut self, cites: impl Into<String>) -> Self {
        self.cites = Some(cites.into());
        self
    }

    /// Only return documents from this domain.
    #[must_use]
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Check the constraints Firecrawl would otherwise reject with a 400.
    pub fn validate(&self) -> Result<()> {
        let invalid =
            |message: &str, param: &str| Err(Error::validation(message, Some(param.to_string())));
        if self.query.trim().is_empty() {
            return invalid("query must not be empty", "query");
        }
        if self.k.is_some_and(|k| !(1..=100).contains(&k)) {
            return invalid("k must be between 1 and 100", "k");
        }
        if let Some(jurisdiction) = &self.jurisdiction
            && !is_jurisdiction(&jurisdiction.to_ascii_lowercase())
        {
            return invalid(
                "jurisdiction must be \"us\" or \"us-\" and a two-letter state code",
                "jurisdiction",
            );
        }
        for (value, param) in [
            (&self.court, "court"),
            (&self.cites, "cites"),
            (&self.domain, "domain"),
        ] {
            if value.as_ref().is_some_and(|v| v.trim().is_empty()) {
                return invalid(&format!("{param} must not be empty"), param);
            }
        }
        Ok(())
    }

    pub(super) fn wire(&self) -> WireGovSearchRequest<'_> {
        WireGovSearchRequest {
            query: &self.query,
            k: self.k,
            jurisdiction: self.jurisdiction.as_deref().map(str::to_ascii_lowercase),
            court: self.court.as_deref(),
            court_type: self.court_type,
            cites: self.cites.as_deref(),
            domain: self.domain.as_deref(),
        }
    }
}

/// `us`, or `us-` and two lowercase letters.
fn is_jurisdiction(value: &str) -> bool {
    match value.strip_prefix("us") {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('-')
            .is_some_and(|code| code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase())),
        None => false,
    }
}

// Firecrawl rejects unknown fields, so only the fields that are set are sent.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireGovSearchRequest<'request> {
    query: &'request str,
    #[serde(skip_serializing_if = "Option::is_none")]
    k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    jurisdiction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    court: Option<&'request str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    court_type: Option<CourtType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cites: Option<&'request str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    domain: Option<&'request str>,
}

/// A successful Government Index search.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GovSearchResponse {
    /// Whether the search succeeded.
    pub success: bool,
    /// The results.
    #[serde(default)]
    pub data: GovSearchData,
}

/// Government Index results.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GovSearchData {
    /// Matching documents, best first.
    #[serde(default)]
    pub web: Vec<GovResult>,
}

/// A Government Index result.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GovResult {
    /// Document URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Document title.
    #[serde(default)]
    pub title: Option<String>,
    /// The passage that matched the query.
    #[serde(default)]
    pub description: Option<String>,
    /// Rank within the results, from 1.
    #[serde(default)]
    pub position: Option<u32>,
}
