//! The client-side `fetch_page` tool backed by Firecrawl's scrape API.

use std::time::Duration;

use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::filter::Filter;
use super::{FirecrawlClient, ScrapeOptions, ScrapeRequest, ScrapeResponse, tool_result};
use crate::client_tool::ClientTool;
use crate::error::{Error, Result};
use crate::types::{TextBlock, ToolParam, ToolResultBlock, ToolUnionParam, ToolUseBlock};

const FETCH_PAGE_TOOL_NAME: &str = "fetch_page";
// Claude cites whole text blocks, so this is also the size of one citation.
const CHUNK_CHARS: usize = 1500;

/// Firecrawl page fetch exposed to the model as a client-side `fetch_page` tool.
///
/// The page comes back as one `search_result` block with citations enabled, its markdown split
/// into blocks of about 1,500 characters at blank lines and headings. Domain lists are checked
/// before the fetch and again against the URL Firecrawl ended on, so a redirect cannot reach a
/// refused domain; a reported URL that is not absolute http(s) is ignored and the requested URL
/// stands in for it. Entries are hostnames that cover their subdomains, as in
/// [`FirecrawlWebSearch`](super::FirecrawlWebSearch).
#[derive(Debug, Clone)]
pub struct FirecrawlFetchPage {
    client: FirecrawlClient,
    max_content_chars: usize,
    scrape_options: ScrapeOptions,
    allowed_domains: Vec<String>,
    blocked_domains: Vec<String>,
}

impl FirecrawlFetchPage {
    /// Wrap a client with the defaults: main content only, cached copies up to an hour old, a
    /// 30 second scrape timeout, at most 20 PDF pages, markdown cut at 30,000 characters.
    #[must_use]
    pub fn new(client: FirecrawlClient) -> Self {
        Self {
            client,
            max_content_chars: 30_000,
            scrape_options: ScrapeOptions::new()
                .with_only_main_content(true)
                .with_max_age(Duration::from_secs(3600))
                .with_timeout(Duration::from_secs(30))
                .with_pdf_max_pages(20),
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
        }
    }

    /// Characters of page markdown kept.
    #[must_use]
    pub fn with_max_content_chars(mut self, max_content_chars: usize) -> Self {
        self.max_content_chars = max_content_chars;
        self
    }

    /// How pages are scraped. Replaces every default, including the PDF page cap that bounds
    /// credits.
    #[must_use]
    pub fn with_scrape_options(mut self, scrape_options: ScrapeOptions) -> Self {
        self.scrape_options = scrape_options;
        self
    }

    /// Only ever fetch pages on these hostnames.
    #[must_use]
    pub fn with_allowed_domains(mut self, domains: Vec<String>) -> Self {
        self.allowed_domains = domains;
        self
    }

    /// Never fetch pages on these hostnames.
    #[must_use]
    pub fn with_blocked_domains(mut self, domains: Vec<String>) -> Self {
        self.blocked_domains = domains;
        self
    }

    fn checked_config(&self) -> Result<Filter> {
        let invalid =
            |message: String, param: &str| Error::validation(message, Some(param.to_string()));
        if self.max_content_chars == 0 {
            return Err(invalid(
                "max_content_chars must be positive".to_string(),
                "max_content_chars",
            ));
        }
        self.scrape_options.validate()?;
        Filter::try_new(&self.allowed_domains, &self.blocked_domains)
    }

    async fn execute(&self, tool_use: &ToolUseBlock) -> Result<ToolResultBlock> {
        let filter = self.checked_config()?;
        let id = tool_use.id.clone();
        let url = match ModelInput::deserialize(&tool_use.input) {
            Ok(input) => input.url.trim().to_string(),
            Err(error) => {
                return Ok(ToolResultBlock::error(
                    id,
                    format!(
                        "Invalid fetch_page input: {error}. Send an object with only a \"url\" \
                         string."
                    ),
                ));
            }
        };
        Ok(match self.fetch(&url, &filter).await? {
            Ok(page) => self.result(id, &url, page),
            Err(message) => ToolResultBlock::error(id, message),
        })
    }

    /// Fetch for the sandbox's `scrape(...)`: the same checks and error mapping as the tool,
    /// returning the page as data. The inner `Err` is a message the model can act on; `Err` is
    /// fatal.
    #[cfg(feature = "code-mode")]
    pub(crate) async fn page_value(
        &self,
        url: &str,
    ) -> Result<std::result::Result<PageData, String>> {
        let filter = self.checked_config()?;
        let url = url.trim();
        Ok(self.fetch(url, &filter).await?.map(|page| PageData {
            url: page.source,
            status: page.status,
            title: page.title,
            markdown: page.markdown,
        }))
    }

    /// Fetches one page and checks where it landed and its status. The inner `Err` is a
    /// message the model can act on; `Err` is fatal (401, 402).
    async fn fetch(&self, url: &str, filter: &Filter) -> Result<std::result::Result<Page, String>> {
        let request = ScrapeRequest::new(url).with_options(self.scrape_options.clone());
        if let Err(error) = request.validate() {
            return Ok(Err(format!("Could not fetch {url}: {error}")));
        }
        if !filter.permits(url) {
            return Ok(Err(format!(
                "Could not fetch {url}: this tool may not fetch pages on that domain."
            )));
        }
        match self.client.scrape(&request).await {
            Ok(response) => Ok(self.page(url, response, filter)),
            Err(error) if matches!(error.status(), Some(401 | 402)) => Err(error),
            Err(error) => Ok(Err(format!("Could not fetch {url}: {error}"))),
        }
    }

    fn page(
        &self,
        url: &str,
        response: ScrapeResponse,
        filter: &Filter,
    ) -> std::result::Result<Page, String> {
        let data = response.data;
        let metadata = data.metadata.unwrap_or_default();
        // Firecrawl can report an empty or relative URL; only an absolute http(s) one counts.
        let landed = metadata.url.filter(|u| is_absolute_http(u));
        if let Some(landed) = landed.as_deref().filter(|u| !filter.permits(u)) {
            return Err(format!(
                "Could not fetch {url}: it redirected to {landed}, a domain this tool may not \
                 fetch."
            ));
        }
        if let Some(status) = metadata.status_code.filter(|s| !is_success(*s)) {
            return Err(format!(
                "Could not fetch {url}: the page returned HTTP {status}."
            ));
        }
        let pdf_note = match (metadata.num_pages, metadata.total_pages) {
            (Some(read), Some(total)) if read < total => {
                Some(format!("Only pages 1 to {read} of {total} were read."))
            }
            _ => None,
        };
        Ok(Page {
            source: landed
                .or(metadata.source_url.filter(|u| is_absolute_http(u)))
                .unwrap_or_else(|| url.to_string()),
            #[cfg(feature = "code-mode")]
            status: metadata.status_code,
            title: metadata.title,
            markdown: data
                .markdown
                .filter(|m| !m.trim().is_empty())
                .map(|m| tool_result::truncate(&m, self.max_content_chars)),
            pdf_note,
            warning: data
                .warning
                .filter(|w| !w.trim().is_empty())
                .map(|w| format!("Firecrawl warning: {w}")),
        })
    }

    fn result(&self, id: String, url: &str, page: Page) -> ToolResultBlock {
        let Some(markdown) = page.markdown else {
            let text = page.warning.map_or_else(
                || format!("{url} has no readable text."),
                |warning| format!("{url} has no readable text. {warning}"),
            );
            return ToolResultBlock::new(id).with_string_content(text);
        };
        let mut content: Vec<TextBlock> =
            chunks(&markdown).into_iter().map(TextBlock::new).collect();
        tool_result::push_text(&mut content, page.pdf_note);
        tool_result::push_text(&mut content, page.warning);
        // A tool result holding a search result may hold nothing else.
        ToolResultBlock::new(id).with_array_content(vec![tool_result::search_result(
            page.source,
            page.title,
            content,
        )])
    }
}

/// One page as the sandbox's `scrape(...)` returns it.
#[cfg(feature = "code-mode")]
#[derive(serde::Serialize)]
pub(crate) struct PageData {
    /// Where the page ended up after redirects.
    url: String,
    status: Option<u16>,
    title: Option<String>,
    /// Cut to the tool's limit.
    markdown: Option<String>,
}

/// A fetched page that passed every check.
struct Page {
    source: String,
    #[cfg(feature = "code-mode")]
    status: Option<u16>,
    title: Option<String>,
    /// Non-empty markdown, cut to the tool's limit.
    markdown: Option<String>,
    pdf_note: Option<String>,
    warning: Option<String>,
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status) || status == 304
}

impl ClientTool for FirecrawlFetchPage {
    fn name(&self) -> &str {
        FETCH_PAGE_TOOL_NAME
    }

    fn definition(&self) -> ToolUnionParam {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute http or https URL of the page."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        });
        let description = format!(
            "Fetch one web page or PDF and read its main content as markdown, cut off after {} \
             characters. Use it to read a page you found with search or one the user gave you, \
             when a search snippet is not enough. Cite the page from the result.",
            self.max_content_chars
        );
        ToolUnionParam::CustomTool(
            ToolParam::new(FETCH_PAGE_TOOL_NAME.to_string(), schema).with_description(description),
        )
    }

    fn call<'a>(&'a self, tool_use: &'a ToolUseBlock) -> BoxFuture<'a, Result<ToolResultBlock>> {
        Box::pin(self.execute(tool_use))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelInput {
    url: String,
}

fn is_absolute_http(url: &str) -> bool {
    url::Url::parse(url)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

/// Splits markdown into blocks of about [`CHUNK_CHARS`] at blank lines, starting a new block at
/// every ATX heading outside a fenced code block.
pub(super) fn chunks(markdown: &str) -> Vec<String> {
    // Each paragraph and whether it starts with a heading.
    let mut paragraphs: Vec<(String, bool)> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut current_heading = false;
    let mut fence: Option<(char, usize)> = None;
    for line in markdown.lines() {
        let heading = fence.is_none() && is_atx_heading(line);
        fence = next_fence(fence, line);
        if (line.trim().is_empty() || heading) && !current.is_empty() {
            paragraphs.push((current.join("\n"), current_heading));
            current.clear();
        }
        if !line.trim().is_empty() {
            if current.is_empty() {
                current_heading = heading;
            }
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push((current.join("\n"), current_heading));
    }

    let mut blocks = Vec::new();
    let mut block = String::new();
    for (paragraph, heading) in paragraphs {
        for (index, piece) in split_long(&paragraph).into_iter().enumerate() {
            let starts_section = index == 0 && heading;
            let overflows = block.chars().count() + 2 + piece.chars().count() > CHUNK_CHARS;
            if !block.is_empty() && (starts_section || overflows) {
                blocks.push(std::mem::take(&mut block));
            }
            if !block.is_empty() {
                block.push_str("\n\n");
            }
            block.push_str(&piece);
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    blocks
}

/// A line opening an ATX heading: up to three spaces, 1 to 6 `#`, then a space or tab.
fn is_atx_heading(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
    (1..=6).contains(&hashes) && matches!(trimmed[hashes..].chars().next(), Some(' ' | '\t'))
}

/// The fence state after `line`: a run of 3 or more backticks or tildes opens a fenced code
/// block, and a run of the same character at least as long, with nothing after it, closes it.
fn next_fence(fence: Option<(char, usize)>, line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return fence;
    }
    let Some(marker) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) else {
        return fence;
    };
    let run = trimmed.len() - trimmed.trim_start_matches(marker).len();
    if run < 3 {
        return fence;
    }
    match fence {
        None => Some((marker, run)),
        Some((open, len)) if open == marker && run >= len && trimmed[run..].trim().is_empty() => {
            None
        }
        open => open,
    }
}

/// A paragraph cut into pieces of at most [`CHUNK_CHARS`], at whitespace where possible. No
/// piece is empty or ends in whitespace.
fn split_long(paragraph: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = paragraph.trim_end();
    while rest.chars().count() > CHUNK_CHARS {
        let limit = rest
            .char_indices()
            .nth(CHUNK_CHARS)
            .map_or(rest.len(), |(i, _)| i);
        // Never cut inside leading whitespace, which would leave an empty piece.
        let end = rest[..limit]
            .rfind(char::is_whitespace)
            .filter(|&i| !rest[..i].trim().is_empty())
            .unwrap_or(limit);
        let piece = rest[..end].trim_end();
        if !piece.trim().is_empty() {
            pieces.push(piece.to_string());
        }
        rest = rest[end..].trim_start();
    }
    if !rest.trim().is_empty() {
        pieces.push(rest.to_string());
    }
    pieces
}
