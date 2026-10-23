//! The `firecrawl` pack: web search, page scraping, the Government Index and site maps inside the
//! sandbox, with optional caps, an observer and a choice of what running out of credits does.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::Date;
use time::macros::format_description;

use crate::code_mode::{Example, HostError, Pack, PyError, host_fn, value};
use crate::error::{Error, Result};
use crate::firecrawl::{
    FirecrawlClient, FirecrawlFetchPage, FirecrawlWebSearch, GovSearchRequest, MapLink, MapRequest,
    PdfMode, PdfPage, ScrapeData, ScrapeOptions, SearchCategory, SearchQuery, TimeRange,
};
use crate::http;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: u32,
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
    #[serde(default)]
    recency: Option<Recency>,
    #[serde(default)]
    sort_by_date: bool,
    #[serde(default)]
    categories: Option<Vec<Category>>,
    #[serde(default = "default_country")]
    country: Option<String>,
    #[serde(default)]
    scrape: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScrapeArgs {
    url: String,
    #[serde(default = "default_max_pages")]
    max_pages: u32,
    #[serde(default = "default_pdf_mode")]
    pdf_mode: PdfModeArg,
    #[serde(default = "default_page_markers")]
    page_markers: bool,
    #[serde(default)]
    links: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GovSearchArgs {
    query: String,
    #[serde(default = "default_k")]
    k: u32,
    #[serde(default)]
    jurisdiction: Option<String>,
    #[serde(default)]
    domain: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MapArgs {
    url: String,
    #[serde(default)]
    search: Option<String>,
    #[serde(default = "default_map_limit")]
    limit: u32,
}

/// `search` as [`firecrawl`] gives it, with the tool's own default limit and country.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BasicSearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    scrape: bool,
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
}

/// `scrape` as [`firecrawl`] gives it, with the tool's own scrape options.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BasicScrapeArgs {
    url: String,
}

fn default_search_limit() -> u32 {
    8
}

fn default_country() -> Option<String> {
    Some("US".to_string())
}

fn default_max_pages() -> u32 {
    30
}

fn default_pdf_mode() -> PdfModeArg {
    PdfModeArg::Auto
}

fn default_page_markers() -> bool {
    true
}

fn default_k() -> u32 {
    10
}

fn default_map_limit() -> u32 {
    100
}

/// `recency`: `"hour"`, `"day"`, `"week"`, `"month"` or `"year"` for that past period
/// (`tbs=qdr:h`, `qdr:d`, `qdr:w`, `qdr:m`, `qdr:y`), or `"YYYY-MM-DD..YYYY-MM-DD"` for an
/// inclusive date range (`tbs=cdr:1,cd_min:MM/DD/YYYY,cd_max:MM/DD/YYYY`).
struct Recency(TimeRange);

impl<'de> Deserialize<'de> for Recency {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let past = match text.as_str() {
            "hour" => Some(TimeRange::PastHour),
            "day" => Some(TimeRange::PastDay),
            "week" => Some(TimeRange::PastWeek),
            "month" => Some(TimeRange::PastMonth),
            "year" => Some(TimeRange::PastYear),
            _ => None,
        };
        past.or_else(|| date_range(&text)).map(Self).ok_or_else(|| {
            D::Error::custom(format!(
                "recency must be hour, day, week, month, year or YYYY-MM-DD..YYYY-MM-DD, not \
                 {text:?}"
            ))
        })
    }
}

/// `YYYY-MM-DD..YYYY-MM-DD` as a custom range.
fn date_range(text: &str) -> Option<TimeRange> {
    let format = format_description!("[year]-[month]-[day]");
    let (from, to) = text.split_once("..")?;
    Some(TimeRange::Custom {
        from: Date::parse(from.trim(), format).ok()?,
        to: Date::parse(to.trim(), format).ok()?,
    })
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Category {
    Developer,
    Research,
    Pdf,
    Gov,
}

impl Category {
    fn category(self) -> SearchCategory {
        match self {
            Self::Developer => SearchCategory::Developer,
            Self::Research => SearchCategory::Research,
            Self::Pdf => SearchCategory::Pdf,
            Self::Gov => SearchCategory::Gov,
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PdfModeArg {
    Fast,
    Auto,
    Ocr,
}

impl PdfModeArg {
    fn mode(self) -> PdfMode {
        match self {
            Self::Fast => PdfMode::Fast,
            Self::Auto => PdfMode::Auto,
            Self::Ocr => PdfMode::Ocr,
        }
    }
}

/// Firecrawl use by one conversation: a snapshot from [`FirecrawlBudget::used`] to save, and to
/// restore with [`FirecrawlBudget::with_used`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FirecrawlUsed {
    /// `search` calls sent.
    pub searches: u32,
    /// `scrape` calls sent.
    pub scrapes: u32,
    /// `map_site` calls sent.
    pub maps: u32,
    /// `gov_search` calls sent.
    pub gov_searches: u32,
    /// Credits charged: what each answered call cost, and the reservation of each sent call
    /// that failed.
    pub credits: u64,
}

/// The calls a [`FirecrawlBudget`] counts.
#[derive(Clone, Copy)]
enum Call {
    Search,
    Scrape,
    Map,
    GovSearch,
}

/// Per-conversation caps on Firecrawl calls and credits, shared by every function of the pack.
///
/// A call counts once it is sent, whether or not it succeeds; client retries are not counted
/// again. A call that fails its checks is refused before sending and not counted. Once a cap is
/// reached, the functions it covers raise `RuntimeError` without sending anything.
///
/// Credits are reserved before sending and settled after the answer. A scrape reserves 1 plus
/// any paid formats and is settled to [`ScrapeData::credits`]: Firecrawl's `creditsUsed`, 0 for
/// a cache hit, else 1 per PDF page. A search reserves 2 per 10 results asked for, plus 1 per
/// result when it scrapes, and is settled to the response's `creditsUsed` (0 for a cache hit),
/// else 2 per 10 results returned (2 for none) plus 1 per scraped one. A map costs 1, and 0 for
/// a cache hit. A gov search is free,
/// so it only counts against its own cap and is never stopped by the credit cap. A sent call
/// that fails keeps its reservation, since Firecrawl may have billed it. A call is let through
/// while the credits charged are under the cap, so the last one can overshoot by its own cost,
/// such as a long PDF.
#[derive(Debug, Default)]
pub struct FirecrawlBudget {
    max_searches: Option<u32>,
    max_scrapes: Option<u32>,
    max_maps: Option<u32>,
    max_gov_searches: Option<u32>,
    max_credits: Option<u64>,
    searches: AtomicU32,
    scrapes: AtomicU32,
    maps: AtomicU32,
    gov_searches: AtomicU32,
    credits: AtomicU64,
}

impl FirecrawlBudget {
    /// A budget with no caps, nothing used yet. It still counts, for [`FirecrawlBudget::used`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cap `search` calls.
    #[must_use]
    pub fn with_max_searches(mut self, max: u32) -> Self {
        self.max_searches = Some(max);
        self
    }

    /// Cap `scrape` calls.
    #[must_use]
    pub fn with_max_scrapes(mut self, max: u32) -> Self {
        self.max_scrapes = Some(max);
        self
    }

    /// Cap `map_site` calls.
    #[must_use]
    pub fn with_max_maps(mut self, max: u32) -> Self {
        self.max_maps = Some(max);
        self
    }

    /// Cap `gov_search` calls.
    #[must_use]
    pub fn with_max_gov_searches(mut self, max: u32) -> Self {
        self.max_gov_searches = Some(max);
        self
    }

    /// Cap the credits charged, across every function.
    #[must_use]
    pub fn with_max_credits(mut self, max: u64) -> Self {
        self.max_credits = Some(max);
        self
    }

    /// Start from a saved snapshot, such as after a restart.
    #[must_use]
    pub fn with_used(self, used: FirecrawlUsed) -> Self {
        self.searches.store(used.searches, Ordering::SeqCst);
        self.scrapes.store(used.scrapes, Ordering::SeqCst);
        self.maps.store(used.maps, Ordering::SeqCst);
        self.gov_searches.store(used.gov_searches, Ordering::SeqCst);
        self.credits.store(used.credits, Ordering::SeqCst);
        self
    }

    /// What has been used so far.
    #[must_use]
    pub fn used(&self) -> FirecrawlUsed {
        FirecrawlUsed {
            searches: self.searches.load(Ordering::SeqCst),
            scrapes: self.scrapes.load(Ordering::SeqCst),
            maps: self.maps.load(Ordering::SeqCst),
            gov_searches: self.gov_searches.load(Ordering::SeqCst),
            credits: self.credits.load(Ordering::SeqCst),
        }
    }

    fn counter(&self, call: Call) -> (&AtomicU32, Option<u32>, &'static str) {
        match call {
            Call::Search => (&self.searches, self.max_searches, "search"),
            Call::Scrape => (&self.scrapes, self.max_scrapes, "scrape"),
            Call::Map => (&self.maps, self.max_maps, "map"),
            Call::GovSearch => (&self.gov_searches, self.max_gov_searches, "gov search"),
        }
    }

    /// Counts one call and reserves `credits` for it, or gives the reason it is refused when a
    /// cap is reached. A call reserving no credits is not checked against the credit cap.
    fn take(&self, call: Call, credits: u64) -> std::result::Result<(), String> {
        let (count, max, noun) = self.counter(call);
        count
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                under(used, max).then(|| used.saturating_add(1))
            })
            .map_err(|_| refusal(noun, max))?;
        if credits == 0 {
            return Ok(());
        }
        self.credits
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                under(used, self.max_credits).then(|| used.saturating_add(credits))
            })
            .map(|_| ())
            .map_err(|_| {
                // The call is not sent, so it does not count.
                count.fetch_sub(1, Ordering::SeqCst);
                refusal("credit", self.max_credits)
            })
    }

    /// Replaces a call's reservation with what it cost.
    fn settle(&self, reserved: u64, cost: u64) {
        if cost >= reserved {
            self.credits.fetch_add(cost - reserved, Ordering::SeqCst);
        } else {
            self.credits.fetch_sub(reserved - cost, Ordering::SeqCst);
        }
    }
}

/// Whether `used` is under `max`, if there is one.
fn under<T: PartialOrd>(used: T, max: Option<T>) -> bool {
    max.is_none_or(|max| used < max)
}

fn refusal<T: std::fmt::Display>(noun: &str, max: Option<T>) -> String {
    let max = max.map_or_else(String::new, |max| max.to_string());
    format!("Firecrawl {noun} budget of {max} used up; finish with what you have")
}

/// What a Firecrawl 402 (out of credits) does in the pack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutOfCredits {
    /// Raise `RuntimeError` in the script, so the run carries on without Firecrawl.
    Raise,
    /// End the run with the error, as the Firecrawl tools do. The default.
    #[default]
    Fatal,
}

/// One search or Government Index result as the sandbox gets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FirecrawlHit {
    /// Page URL.
    pub url: String,
    /// Page title.
    pub title: Option<String>,
    /// The passage that best answers the query.
    pub description: Option<String>,
    /// Rank within the results.
    pub position: Option<u32>,
    /// The page as markdown, cut to the pack's limit; only from `search(scrape=True)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// The page's HTTP status; only from `search(scrape=True)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

/// One scraped page as the sandbox gets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct FirecrawlPage {
    /// Where the page ended up after redirects.
    pub url: String,
    /// The URL the script asked for.
    pub requested_url: String,
    /// The page's HTTP status.
    pub status: Option<u16>,
    /// Page title.
    pub title: Option<String>,
    /// The page as markdown, cut to the pack's limit; `None` when it has no text.
    pub markdown: Option<String>,
    /// Each PDF page's markdown, as `{"page": n, "markdown": text}` in Python, when the scrape
    /// defaults ask for pages. Cut to the same limit as `markdown`, all pages together, so with
    /// pages on one result can carry up to twice the content limit.
    #[serde(serialize_with = "page_dicts")]
    pub pages: Option<Vec<PdfPage>>,
    /// Links on the page, when the script asked for them.
    pub links: Option<Vec<String>>,
    /// For a PDF, the pages read.
    pub num_pages: Option<u32>,
    /// For a PDF, its full page count, when Firecrawl knows it.
    pub total_pages: Option<u32>,
    /// Characters of `markdown` before the cut.
    pub chars: usize,
    /// Characters of all `pages` together before the cut; `None` when there are no pages.
    pub page_chars: Option<usize>,
    /// Whether `markdown` or `pages` was cut.
    pub truncated: bool,
    /// The PDF page limit used, when [`FirecrawlPack::with_max_pdf_pages`] lowered the
    /// script's `max_pages`.
    pub clamped_to: Option<u32>,
    /// Credits the scrape cost, from [`ScrapeData::credits`].
    pub credits: u64,
}

fn page_dicts<S: Serializer>(
    pages: &Option<Vec<PdfPage>>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    pages
        .as_ref()
        .map(|pages| {
            pages
                .iter()
                .map(|page| serde_json::json!({"page": page.page, "markdown": page.markdown}))
                .collect::<Vec<_>>()
        })
        .serialize(serializer)
}

/// What the pack did for one call, in order.
///
/// A sent call gives `Searching`, `Scraping` or `Mapping`, then its result event or `Failed`.
/// A call that fails its checks gives only `Failed`, and one stopped by the budget only
/// `Refused`. A call whose arguments do not fit the function raises `TypeError` before the pack
/// runs and gives no event. A call dropped by the host-call timeout while its request is in
/// flight may give the first event with nothing after it. `function` is the Python function
/// called:
/// `"search"`, `"scrape"`, `"gov_search"` or `"map_site"`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FirecrawlEvent {
    /// A search or gov search is about to be sent, after its checks and the budget.
    Searching {
        /// The Python function called.
        function: &'static str,
        /// The query.
        query: String,
    },
    /// A search answered.
    Searched {
        /// The Python function called.
        function: &'static str,
        /// The query.
        query: String,
        /// The results the script gets.
        hits: Vec<FirecrawlHit>,
        /// Credits it cost.
        credits: u64,
    },
    /// A gov search answered. It costs no credits.
    GovSearched {
        /// The Python function called.
        function: &'static str,
        /// The query.
        query: String,
        /// The results the script gets.
        hits: Vec<FirecrawlHit>,
    },
    /// A scrape is about to be sent, after its checks and the budget.
    Scraping {
        /// The Python function called.
        function: &'static str,
        /// The URL asked for.
        url: String,
    },
    /// A scrape answered.
    Scraped {
        /// The Python function called.
        function: &'static str,
        /// The URL asked for.
        requested: String,
        /// The page as the script gets it, after the cut.
        page: FirecrawlPage,
        /// Credits it cost.
        credits: u64,
    },
    /// A map is about to be sent, after its checks and the budget.
    Mapping {
        /// The Python function called.
        function: &'static str,
        /// The site.
        url: String,
    },
    /// A map answered.
    Mapped {
        /// The Python function called.
        function: &'static str,
        /// The site.
        url: String,
        /// The links the script gets.
        links: Vec<MapLink>,
        /// Credits it cost.
        credits: u64,
    },
    /// The call failed its checks, or its request failed.
    Failed {
        /// The Python function called.
        function: &'static str,
        /// The query or URL.
        arg: String,
        /// The message raised to the script, or the error that ended the run.
        message: String,
    },
    /// A cap was reached, so nothing was sent.
    Refused {
        /// The Python function called.
        function: &'static str,
        /// The message raised to the script.
        reason: String,
    },
}

/// Receives the [`FirecrawlPack`]'s events, for logging or accounting.
pub trait FirecrawlObserver: Send + Sync {
    /// Called once per event, on the task running the host function. Keep it quick.
    fn event(&self, event: FirecrawlEvent);
}

/// The `firecrawl` pack: `search`, `scrape`, `gov_search` and `map_site` over one Firecrawl
/// client.
///
/// Each call checks its arguments, takes its share of the budget, then sends. The pack has no
/// concurrency limit of its own: [`FirecrawlClient::with_max_concurrency`] covers every request
/// the client sends. A call waiting there for a slot has already been counted, so if the
/// host-call timeout drops it while it waits, it still counts.
///
/// Errors map as the Firecrawl tools map them: a problem the script can handle raises
/// `RuntimeError`, an invalid key (401) ends the run, and a 402 does what
/// [`FirecrawlPack::with_out_of_credits`] says. Build a new pack for each conversation.
///
/// ```no_run
/// # fn build(client: topcoat_si::FirecrawlClient) {
/// use std::sync::Arc;
///
/// use topcoat_si::code_mode::packs::{FirecrawlBudget, FirecrawlPack, OutOfCredits};
///
/// let budget = Arc::new(
///     FirecrawlBudget::new()
///         .with_max_searches(30)
///         .with_max_scrapes(25)
///         .with_max_credits(600),
/// );
/// let pack = FirecrawlPack::new(client.with_max_concurrency(10))
///     .with_budget(budget.clone())
///     .with_max_pdf_pages(60)
///     .with_out_of_credits(OutOfCredits::Raise)
///     .build();
/// // ... later, save `budget.used()` with the conversation ...
/// # }
/// ```
pub struct FirecrawlPack {
    search: FirecrawlWebSearch,
    fetch: FirecrawlFetchPage,
    budget: Option<Arc<FirecrawlBudget>>,
    observer: Option<Arc<dyn FirecrawlObserver>>,
    out_of_credits: OutOfCredits,
    max_pdf_pages: Option<u32>,
}

impl FirecrawlPack {
    /// A pack over `client` with the Firecrawl tools' defaults, no caps, no observer, and a 402
    /// that ends the run.
    #[must_use]
    pub fn new(client: FirecrawlClient) -> Self {
        Self::from_tools(
            FirecrawlWebSearch::new(client.clone()),
            FirecrawlFetchPage::new(client),
        )
    }

    /// A pack over the app's own tools, so their domain lists, scrape options and content cut
    /// apply as in the tool loop. `search` also checks gov search results; `fetch` also checks
    /// the site and the links of a map.
    #[must_use]
    pub fn from_tools(search: FirecrawlWebSearch, fetch: FirecrawlFetchPage) -> Self {
        Self {
            search,
            fetch,
            budget: None,
            observer: None,
            out_of_credits: OutOfCredits::default(),
            max_pdf_pages: None,
        }
    }

    /// Cap calls and credits. Keep the `Arc` to read [`FirecrawlBudget::used`] later.
    #[must_use]
    pub fn with_budget(mut self, budget: Arc<FirecrawlBudget>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// How `scrape` scrapes pages, before the script's `max_pages`, `pdf_mode`, `page_markers`
    /// and `links` apply. Replaces the fetch tool's options. Ask for
    /// [`ScrapeOptions::with_pdf_pages`] here to fill `pages`.
    #[must_use]
    pub fn with_scrape_defaults(mut self, options: ScrapeOptions) -> Self {
        self.fetch = self.fetch.with_scrape_options(options);
        self
    }

    /// Read at most `n` PDF pages per scrape, whatever `max_pages` the script asks for.
    #[must_use]
    pub fn with_max_pdf_pages(mut self, n: u32) -> Self {
        self.max_pdf_pages = Some(n);
        self
    }

    /// Characters of markdown kept per page, for `scrape` and for `search(scrape=True)`.
    #[must_use]
    pub fn with_content_chars(mut self, n: usize) -> Self {
        self.search = self.search.with_max_content_chars(n);
        self.fetch = self.fetch.with_max_content_chars(n);
        self
    }

    /// Send every [`FirecrawlEvent`] to `observer`.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn FirecrawlObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// What a Firecrawl 402 does. Defaults to [`OutOfCredits::Fatal`].
    #[must_use]
    pub fn with_out_of_credits(mut self, out_of_credits: OutOfCredits) -> Self {
        self.out_of_credits = out_of_credits;
        self
    }

    /// The pack.
    #[must_use]
    pub fn build(self) -> Pack {
        let crawler = Arc::new(self.crawler());
        let (scraper, gov, mapper) = (crawler.clone(), crawler.clone(), crawler.clone());
        Pack::new(
            "firecrawl",
            "web search, page scraping, the Government Index and site maps",
        )
        .with_host(host_fn(
            "search",
            "search(*, query: str, limit: int = 8, include_domains: list[str] | None = None, \
             exclude_domains: list[str] | None = None, recency: str | None = None, \
             sort_by_date: bool = False, categories: list[str] | None = None, country: str = \
             \"US\", scrape: bool = False) -> list[dict]",
            "search the web, at most 10 results; results have url, title, description (the \
             passage that best answers the query) and position, and with scrape=True markdown \
             and status; country=None sends no country, or the search tool's own one; recency is hour, day, week, month, year or YYYY-MM-DD..YYYY-MM-DD; \
             categories from gov, pdf, research, developer",
            move |args: SearchArgs| {
                let crawler = crawler.clone();
                async move {
                    let query = SearchQuery {
                        query: args.query,
                        limit: Some(args.limit),
                        scrape: args.scrape,
                        include_domains: args.include_domains,
                        exclude_domains: args.exclude_domains,
                        time_range: args.recency.map(|recency| recency.0),
                        sort_by_date: args.sort_by_date,
                        categories: args
                            .categories
                            .unwrap_or_default()
                            .into_iter()
                            .map(Category::category)
                            .collect(),
                        country: args.country,
                    };
                    value::to_json(crawler.search(query).await?)
                }
            },
        ))
        .with_host(host_fn(
            "scrape",
            "scrape(*, url: str, max_pages: int = 30, pdf_mode: str = \"auto\", page_markers: \
             bool = True, links: bool = False) -> dict",
            "one page or PDF as markdown; returns url (final), requested_url, status, title, \
             markdown, pages, links (with links=True), num_pages and total_pages (PDF pages \
             read and in all), chars and page_chars (markdown and pages before the cut), \
             truncated (either was cut), clamped_to (the page limit used, when lower than \
             max_pages) and credits; markdown and pages are each cut to the content limit, so \
             with pages on one result can carry twice that; page_markers puts <!-- page N --> \
             before each PDF page after the first; pdf_mode is fast, auto or ocr",
            move |args: ScrapeArgs| {
                let crawler = scraper.clone();
                async move {
                    let max_pages = crawler
                        .max_pdf_pages
                        .map_or(args.max_pages, |cap| args.max_pages.min(cap));
                    let options = crawler
                        .fetch
                        .scrape_options()
                        .clone()
                        .with_pdf_max_pages(max_pages)
                        .with_pdf_mode(args.pdf_mode.mode())
                        .with_pdf_page_markers(args.page_markers)
                        .with_links(args.links);
                    let clamped_to = (max_pages < args.max_pages).then_some(max_pages);
                    value::to_json(crawler.scrape(&args.url, options, clamped_to).await?)
                }
            },
        ))
        .with_host(host_fn(
            "gov_search",
            "gov_search(*, query: str, k: int = 10, jurisdiction: str | None = None, domain: \
             str | None = None) -> list[dict]",
            "search the Government Index of statutes, codes, regulations and court opinions, \
             free; jurisdiction is \"us\" or \"us-\" and a state code such as \"us-tn\" (no \
             city or county filter); results have url, title, description and position",
            move |args: GovSearchArgs| {
                let crawler = gov.clone();
                async move {
                    let mut request = GovSearchRequest::new(args.query).with_k(args.k);
                    request.jurisdiction = args.jurisdiction;
                    request.domain = args.domain;
                    value::to_json(crawler.gov_search(request).await?)
                }
            },
        ))
        .with_host(host_fn(
            "map_site",
            "map_site(*, url: str, search: str | None = None, limit: int = 100) -> list[dict]",
            "list a site's URLs from its sitemap and links, most relevant to search first; \
             links have url, title and description; 1 credit",
            move |args: MapArgs| {
                let crawler = mapper.clone();
                async move {
                    let mut request = MapRequest::new(args.url).with_limit(args.limit);
                    request.search = args.search;
                    value::to_json(crawler.map(request).await?)
                }
            },
        ))
        .with_example(
            "search",
            Example {
                code: r#"[hit["url"] for hit in await search(query="rezoning hearing", limit=2, recency="year")]"#,
                expect: "=> ['https://example.com/a', 'https://example.com/b']",
            },
        )
        .with_example(
            "scrape",
            Example {
                code: r#"page = await scrape(url="https://example.com/agenda.pdf", pdf_mode="fast")
(page["num_pages"], page["credits"], page["markdown"])"#,
                expect: "=> (2, 2, 'Item 1<!-- page 2 -->Item 2')",
            },
        )
        .with_example(
            "gov_search",
            Example {
                code: r#"[hit["url"] for hit in await gov_search(query="zoning variance", jurisdiction="us-tn")]"#,
                expect: "=> ['https://law.example.gov/zoning']",
            },
        )
        .with_example(
            "map_site",
            Example {
                code: r#"[link["url"] for link in await map_site(url="https://example.gov", search="agenda", limit=2)]"#,
                expect: "=> ['https://example.gov/agendas', 'https://example.gov/minutes']",
            },
        )
    }

    fn crawler(self) -> Crawler {
        Crawler {
            search: self.search,
            fetch: self.fetch,
            budget: self.budget,
            observer: self.observer,
            out_of_credits: self.out_of_credits,
            max_pdf_pages: self.max_pdf_pages,
        }
    }
}

/// `search` and `scrape`, built from the app's tools so their domain lists, the 10-result cap,
/// scrape options and content cut apply exactly as in the tool loop.
///
/// `max_credits` is a per-conversation Firecrawl credit budget, charged as
/// [`FirecrawlBudget`] charges it: what each call really cost ([`ScrapeData::credits`], a
/// search's `creditsUsed` or the fallback), and the reservation of a sent call that failed. Once
/// reached, both functions raise `RuntimeError`. A 402 ends the run. Build a new pack for each conversation; [`FirecrawlPack`] adds `gov_search`,
/// `map_site`, more search and scrape options, call caps and an observer.
#[must_use]
pub fn firecrawl(search: FirecrawlWebSearch, fetch: FirecrawlFetchPage, max_credits: u32) -> Pack {
    let budget = FirecrawlBudget::new().with_max_credits(u64::from(max_credits));
    let crawler = Arc::new(
        FirecrawlPack::from_tools(search, fetch)
            .with_budget(Arc::new(budget))
            .crawler(),
    );
    let scraper = crawler.clone();
    Pack::new("firecrawl", "web search and page scraping")
        .with_host(host_fn(
            "search",
            "search(*, query: str, limit: int = 5, scrape: bool = False, include_domains: \
             list[str] | None = None, exclude_domains: list[str] | None = None) -> list[dict]",
            "search the web; results have url, title, description, position and, with \
             scrape=True, markdown and status",
            move |args: BasicSearchArgs| {
                let crawler = crawler.clone();
                async move {
                    let query = SearchQuery {
                        query: args.query,
                        limit: args.limit,
                        scrape: args.scrape,
                        include_domains: args.include_domains,
                        exclude_domains: args.exclude_domains,
                        time_range: None,
                        sort_by_date: false,
                        categories: Vec::new(),
                        country: None,
                    };
                    value::to_json(crawler.search(query).await?)
                }
            },
        ))
        .with_host(host_fn(
            "scrape",
            "scrape(*, url: str) -> dict",
            "one page as markdown, with its final url, status and title",
            move |args: BasicScrapeArgs| {
                let crawler = scraper.clone();
                async move {
                    let options = crawler.fetch.scrape_options().clone();
                    value::to_json(crawler.scrape(&args.url, options, None).await?)
                }
            },
        ))
        .with_example(
            "search",
            Example {
                code: r#"[hit["url"] for hit in await search(query="rust async", limit=2)]"#,
                expect: "=> ['https://example.com/a', 'https://example.com/b']",
            },
        )
        .with_example(
            "scrape",
            Example {
                code: r#"(await scrape(url="https://example.com/a"))["title"]"#,
                expect: "=> 'Page A'",
            },
        )
}

/// The shared path of every function: checks, budget, observer, then the request.
struct Crawler {
    search: FirecrawlWebSearch,
    fetch: FirecrawlFetchPage,
    budget: Option<Arc<FirecrawlBudget>>,
    observer: Option<Arc<dyn FirecrawlObserver>>,
    out_of_credits: OutOfCredits,
    max_pdf_pages: Option<u32>,
}

impl Crawler {
    async fn search(
        &self,
        query: SearchQuery,
    ) -> std::result::Result<Vec<FirecrawlHit>, HostError> {
        const FUNCTION: &str = "search";
        let text = query.query.clone();
        let raised = |error: &Error| format!("Web search failed: {error}");
        let prepared = self.mapped(FUNCTION, &text, self.search.prepare_search(query), raised)?;
        let reserved = prepared.estimated_credits();
        self.take(FUNCTION, Call::Search, reserved)?;
        self.notify(FirecrawlEvent::Searching {
            function: FUNCTION,
            query: text.clone(),
        });
        let searched = self.search.send_search(prepared).await;
        let searched = self.mapped(FUNCTION, &text, searched, raised)?;
        self.settle(reserved, searched.credits);
        let hits: Vec<FirecrawlHit> = searched
            .results
            .into_iter()
            .filter_map(|web| {
                Some(FirecrawlHit {
                    url: web.url?,
                    title: web.title,
                    description: web.description,
                    position: web.position,
                    markdown: web.markdown,
                    status: web.metadata.and_then(|m| m.status_code),
                })
            })
            .collect();
        self.notify(FirecrawlEvent::Searched {
            function: FUNCTION,
            query: text,
            hits: hits.clone(),
            credits: searched.credits,
        });
        Ok(hits)
    }

    async fn scrape(
        &self,
        url: &str,
        options: ScrapeOptions,
        clamped_to: Option<u32>,
    ) -> std::result::Result<FirecrawlPage, HostError> {
        const FUNCTION: &str = "scrape";
        let url = url.trim();
        let raised = |error: &Error| format!("Could not fetch {url}: {error}");
        let prepared = self.fetch.prepare_page(url, options.clone());
        let request = self.mapped(FUNCTION, url, prepared, raised)?;
        let reserved = ScrapeData::default().credits(&options);
        self.take(FUNCTION, Call::Scrape, reserved)?;
        self.notify(FirecrawlEvent::Scraping {
            function: FUNCTION,
            url: url.to_string(),
        });
        let sent = self.fetch.send_page(&request).await;
        let (landed, data) = self.mapped(FUNCTION, url, sent, raised)?;
        let credits = data.credits(&options);
        self.settle(reserved, credits);
        let max_chars = self.fetch.max_content_chars();
        let page = cut_page(landed, url, data, max_chars, clamped_to, credits);
        self.notify(FirecrawlEvent::Scraped {
            function: FUNCTION,
            requested: url.to_string(),
            page: page.clone(),
            credits,
        });
        Ok(page)
    }

    async fn gov_search(
        &self,
        request: GovSearchRequest,
    ) -> std::result::Result<Vec<FirecrawlHit>, HostError> {
        const FUNCTION: &str = "gov_search";
        let query = request.query.clone();
        let raised = |error: &Error| format!("Gov search failed: {error}");
        let prepared = self.search.prepare_gov_search(request);
        let request = self.mapped(FUNCTION, &query, prepared, raised)?;
        self.take(FUNCTION, Call::GovSearch, 0)?;
        self.notify(FirecrawlEvent::Searching {
            function: FUNCTION,
            query: query.clone(),
        });
        let found = self.search.send_gov_search(&request).await;
        let hits: Vec<FirecrawlHit> = self
            .mapped(FUNCTION, &query, found, raised)?
            .into_iter()
            .filter_map(|result| {
                Some(FirecrawlHit {
                    url: result.url?,
                    title: result.title,
                    description: result.description,
                    position: result.position,
                    markdown: None,
                    status: None,
                })
            })
            .collect();
        self.notify(FirecrawlEvent::GovSearched {
            function: FUNCTION,
            query,
            hits: hits.clone(),
        });
        Ok(hits)
    }

    async fn map(&self, request: MapRequest) -> std::result::Result<Vec<MapLink>, HostError> {
        const FUNCTION: &str = "map_site";
        // Reserve the 1 credit Firecrawl bills a map, however many links come back; a cache
        // hit settles to 0.
        const RESERVED: u64 = 1;
        let url = request.url.trim().to_string();
        let raised = |error: &Error| format!("Could not map {url}: {error}");
        let prepared = self.fetch.prepare_map(request);
        let request = self.mapped(FUNCTION, &url, prepared, raised)?;
        self.take(FUNCTION, Call::Map, RESERVED)?;
        self.notify(FirecrawlEvent::Mapping {
            function: FUNCTION,
            url: url.clone(),
        });
        let mapped = self.fetch.send_map(&request).await;
        let (links, credits) = self.mapped(FUNCTION, &url, mapped, raised)?;
        self.settle(RESERVED, credits);
        self.notify(FirecrawlEvent::Mapped {
            function: FUNCTION,
            url,
            links: links.clone(),
            credits,
        });
        Ok(links)
    }

    /// Takes one call and `credits` from the budget, or reports the refusal and raises it.
    fn take(
        &self,
        function: &'static str,
        call: Call,
        credits: u64,
    ) -> std::result::Result<(), HostError> {
        let Some(budget) = &self.budget else {
            return Ok(());
        };
        budget.take(call, credits).map_err(|reason| {
            self.notify(FirecrawlEvent::Refused {
                function,
                reason: reason.clone(),
            });
            HostError::Raise {
                kind: PyError::RuntimeError,
                message: reason,
            }
        })
    }

    fn settle(&self, reserved: u64, cost: u64) {
        if let Some(budget) = &self.budget {
            budget.settle(reserved, cost);
        }
    }

    /// Maps a tool's outcome, reporting any failure: a message raises `RuntimeError`, a 402
    /// raises `raised(error)` under [`OutOfCredits::Raise`], and any other error ends the run.
    fn mapped<T>(
        &self,
        function: &'static str,
        arg: &str,
        outcome: Result<std::result::Result<T, String>>,
        raised: impl FnOnce(&Error) -> String,
    ) -> std::result::Result<T, HostError> {
        let (message, fatal) = match outcome {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(message)) => (message, None),
            Err(error)
                if error.status() == Some(402) && self.out_of_credits == OutOfCredits::Raise =>
            {
                (raised(&error), None)
            }
            Err(error) => (error.to_string(), Some(error)),
        };
        self.notify(FirecrawlEvent::Failed {
            function,
            arg: arg.to_string(),
            message: message.clone(),
        });
        Err(match fatal {
            Some(error) => HostError::Fatal(error),
            None => HostError::Raise {
                kind: PyError::RuntimeError,
                message,
            },
        })
    }

    fn notify(&self, event: FirecrawlEvent) {
        if let Some(observer) = &self.observer {
            observer.event(event);
        }
    }
}

/// A scraped page as the script gets it: the markdown cut to `max_chars`, and the pages cut to
/// `max_chars` together.
fn cut_page(
    landed: String,
    requested: &str,
    data: ScrapeData,
    max_chars: usize,
    clamped_to: Option<u32>,
    credits: u64,
) -> FirecrawlPage {
    const SUFFIX: &str = " [truncated]";
    let metadata = data.metadata.unwrap_or_default();
    let markdown = data.markdown.filter(|m| !m.trim().is_empty());
    let chars = markdown.as_ref().map_or(0, |m| m.chars().count());
    let mut truncated = chars > max_chars;
    let markdown = markdown.map(|m| http::truncate_chars(&m, max_chars, SUFFIX));
    let page_chars = data
        .pages
        .as_ref()
        .map(|pages| pages.iter().map(|page| page.markdown.chars().count()).sum());
    let pages = data.pages.map(|pages| {
        let mut left = max_chars;
        let mut kept = Vec::new();
        for mut page in pages {
            let length = page.markdown.chars().count();
            if length > left {
                truncated = true;
                if left > 0 {
                    page.markdown = http::truncate_chars(&page.markdown, left, SUFFIX);
                    kept.push(page);
                }
                break;
            }
            left -= length;
            kept.push(page);
        }
        kept
    });
    FirecrawlPage {
        url: landed,
        requested_url: requested.to_string(),
        status: metadata.status_code,
        title: metadata.title,
        markdown,
        pages,
        links: data.links,
        num_pages: metadata.num_pages,
        total_pages: metadata.total_pages,
        chars,
        page_chars,
        truncated,
        clamped_to,
        credits,
    }
}
