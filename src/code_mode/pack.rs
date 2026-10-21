//! Packs: named sets of functions for the sandbox, and their example calls.

use super::HostFunction;

/// One example call: `code` run in a fresh session must produce exactly `expect`, the text of
/// the tool result.
#[derive(Debug, Clone, Copy)]
pub struct Example {
    /// The snippet.
    pub code: &'static str,
    /// The tool result text, such as `=> 2`.
    pub expect: &'static str,
}

pub(super) enum Function {
    Host(Box<dyn HostFunction>),
    Python {
        name: String,
        signature: String,
        summary: String,
    },
}

impl Function {
    pub(super) fn name(&self) -> &str {
        match self {
            Self::Host(function) => function.name(),
            Self::Python { name, .. } => name,
        }
    }

    pub(super) fn summary(&self) -> &str {
        match self {
            Self::Host(function) => function.summary(),
            Self::Python { summary, .. } => summary,
        }
    }

    /// The full `def` line, without the colon.
    pub(super) fn header(&self) -> String {
        match self {
            Self::Host(function) => format!("async def {}", function.signature()),
            Self::Python { signature, .. } => signature.clone(),
        }
    }

    /// The description line: the signature with types removed and defaults kept.
    pub(super) fn listing(&self) -> String {
        match self {
            Self::Host(function) => untyped(function.signature()),
            Self::Python { signature, .. } => signature.strip_prefix("async def ").map_or_else(
                || untyped(signature.strip_prefix("def ").unwrap_or(signature)),
                |rest| format!("async {}", untyped(rest)),
            ),
        }
    }
}

/// A named set of functions for the sandbox: host functions, Python source, or both.
///
/// Built-in and app packs use this same type. Pack source must stay inside Monty's subset: plain
/// functions, dicts, lists, `async def` with `await` and `asyncio.gather`. No class
/// inheritance, generators, `del`, `async for`/`async with`, or wildcard imports.
pub struct Pack {
    pub(super) name: &'static str,
    pub(super) summary: &'static str,
    pub(super) source: String,
    pub(super) functions: Vec<Function>,
    pub(super) examples: Vec<(String, Example)>,
    pub(super) requires: Vec<&'static str>,
}

impl Pack {
    /// An empty pack; `summary` heads its functions in the tool description.
    #[must_use]
    pub fn new(name: &'static str, summary: &'static str) -> Self {
        Self {
            name,
            summary,
            source: String::new(),
            functions: Vec::new(),
            examples: Vec::new(),
            requires: Vec::new(),
        }
    }

    /// Python source run once when a session starts, after the host functions exist.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// A Python function the source defines, listed in the tool description. `signature` is
    /// its whole `def` or `async def` line without the colon:
    /// `async def scrape_many(*, urls: list[str]) -> list[dict]`.
    #[must_use]
    pub fn with_python_fn(mut self, name: &str, signature: &str, summary: &str) -> Self {
        self.functions.push(Function::Python {
            name: name.to_string(),
            signature: signature.to_string(),
            summary: summary.to_string(),
        });
        self
    }

    /// Adds a function the sandbox can call. Its name must be unique across the packs given
    /// to [`CodeMode`](super::CodeMode)'s constructor, which fails otherwise.
    #[must_use]
    pub fn with_host(mut self, function: impl HostFunction + 'static) -> Self {
        self.functions.push(Function::Host(Box::new(function)));
        self
    }

    /// Attaches an example call for `function`, shown by `help()` and run by
    /// [`testing::check_pack`](super::testing::check_pack). This pack must define `function`, or [`CodeMode`](super::CodeMode)'s constructor
    /// fails.
    #[must_use]
    pub fn with_example(mut self, function: &str, example: Example) -> Self {
        self.examples.push((function.to_string(), example));
        self
    }

    /// Other packs that must be in the same CodeMode, by name.
    #[must_use]
    pub fn requires(mut self, packs: impl IntoIterator<Item = &'static str>) -> Self {
        self.requires.extend(packs);
        self
    }
}

/// `name(*, a: int = 1, b: list[str]) -> T` becomes `name(*, a=1, b) -> T`.
pub(super) fn untyped(signature: &str) -> String {
    let Some(open) = signature.find('(') else {
        return signature.to_string();
    };
    let Some(close) = matching_paren(signature, open) else {
        return signature.to_string();
    };
    let params: Vec<String> = split_top_level(&signature[open + 1..close], ',')
        .into_iter()
        .map(str::trim)
        .filter(|param| !param.is_empty())
        .map(|param| {
            let assignment = split_top_level(param, '=');
            let name = split_top_level(assignment[0], ':')[0].trim();
            assignment.get(1).map_or_else(
                || name.to_string(),
                |default| format!("{name}={}", default.trim()),
            )
        })
        .collect();
    format!(
        "{}({}){}",
        &signature[..open],
        params.join(", "),
        &signature[close + 1..]
    )
}

pub(super) fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut quote = None;
    let from_open = text.char_indices().skip_while(|(i, _)| *i < open);
    for (index, c) in from_open {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits at `separator` outside brackets and quotes.
pub(super) fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut quote = None;
    let mut start = 0;
    for (index, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth = depth.saturating_sub(1),
            (None, c) if c == separator && depth == 0 => {
                parts.push(&text[start..index]);
                start = index + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}
