# Code mode: spec

Claude writes Python; a Monty sandbox runs it; calls to host functions inside the script are
answered by Rust. Exposed to `run_tools` as one client tool, `python`. Functions come in packs:
built-in ones for Firecrawl, Jev, research, triage and reports, and any the app adds.

## Goals

- One Claude turn can run a whole pipeline (search, scrape, classify, filter, report) without
  intermediate data entering Claude's context.
- Apps add their own packs (for example `orders(since=...)` over their own database) through the
  same API the built-in packs use.
- Every pack is tested against a real Monty worker with fake host functions.

## Non-goals

- Anthropic's server-side code execution or programmatic tool calling (`allowed_callers`).
- Write actions with human confirmation. Packs in this version are read-only by contract.
- Filesystem mounts, network access or third-party Python packages in the sandbox.
- Snapshots (`Checkout::dump` / `restore`).
- A README section or example program.

## Dependencies and feature

- Cargo feature `code-mode`, off by default. It enables optional deps `monty-pool = "1.2"` and
  `monty-types = "1.2"` and the module `src/code_mode/`.
- The worker binary is `monty` from `monty-runtime` 1.2, built with
  `cargo install monty-runtime --version 1.2.0 --locked` (without `--locked` it fails to compile
  against a newer `ruff_python_ast`). The app passes its path; this crate never locates or
  downloads it.
- Always the `monty-pool` subprocess transport, never in-process `monty`: model-written code runs
  in a separate, crash-isolated process.

## Ownership: one pool per app, one `CodeMode` per conversation

A session keeps Python variables between calls, so **one `CodeMode` belongs to one
conversation**. Sharing one across conversations would let one user's data reach another's
Claude; the `CodeMode` doc comment says so.

Workers come from one `Arc<Pool>` the app shares across all conversations, so `max_workers` is the
global cap on live sandboxes. A conversation that finds every worker busy waits up to
`checkout_timeout`, then gets a "no sandbox free" tool result.

Each live worker can use up to `max_memory` + 4 MiB (Monty's allocator headroom with type
checking off). With the defaults below that is 68 MiB per worker; `max_workers = 4` bounds the
total near 280 MiB, fine on a 2 GB VM.

## Public API

```rust
pub mod code_mode {
    /// Creates the shared worker pool. `monty` is the path to the monty-runtime 1.2 binary.
    pub async fn pool(monty: impl Into<PathBuf>, max_workers: usize) -> Result<Arc<Pool>>;

    pub struct CodeMode { /* pool, packs, limits, session */ }
    pub struct Pack { /* name, summary, source, functions, examples, requires */ }
    pub trait HostFunction: Send + Sync { /* see below */ }
    pub fn host_fn(/* name, signature, summary, async closure */) -> impl HostFunction;
    pub enum HostError { BadArgs(String), Raise { kind: PyError, message: String }, Fatal(crate::Error) }
    pub enum PyError { ValueError, TypeError, KeyError, LookupError, RuntimeError }
    pub struct Example { pub code: &'static str, pub expect: &'static str }
    pub struct Limits { /* see Defaults */ }
    pub mod packs { /* firecrawl, jev, research, triage, reports */ }
    pub mod testing { /* check_pack */ }
}
```

`pool` uses `PoolConfig::subprocess(monty)` with `min_processes = 1`,
`max_processes = max_workers`, `checkout_timeout = 10s`, and `request_timeout` = the 120 s
`python` call timeout, so a hung worker is always killed. The pool's own backstops already kill a
worker shortly past `max_feed_duration` or `max_turn_duration`; `request_timeout` guards the
requests those limits do not cover. `CodeMode::new` rejects limits where the shorter of
`max_feed_duration` and `max_turn_duration`, plus 5 s, passes 120 s.

`PyError` has no `TimeoutError`: that exception is reserved for the sandbox's own time limit, which
discards the session (Session lifecycle step 4), so a host function raising it would look the
same.

### `HostFunction`

```rust
/// A function the sandbox can call. Must be read-only: the crate cannot enforce this.
pub trait HostFunction: Send + Sync {
    /// Python name, unique across all packs in one CodeMode.
    fn name(&self) -> &str;
    /// The Python signature after `async def`, keyword-only, with a return annotation:
    /// `search(*, query: str, limit: int = 5, scrape: bool = False) -> list[dict]`.
    fn signature(&self) -> &str;
    /// One line, shown in the tool description.
    fn summary(&self) -> &str;
    /// Runs one call. `args` is a JSON object of the keyword arguments as passed.
    fn call<'a>(&'a self, args: serde_json::Value) -> BoxFuture<'a, Result<Value, HostError>>;
}

pub enum HostError {
    /// The arguments don't fit the function: Python `TypeError`, not charged to the budget.
    BadArgs(String),
    /// A problem Claude or the script can handle: becomes this Python exception.
    Raise { kind: PyError, message: String },
    /// A problem nobody in the sandbox can fix: ends the run.
    Fatal(crate::Error),
}
```

- Every host function is **async and keyword-only**. The signature is documentation for Claude
  (tool description and `help()`); nothing type-checks snippets (see Type checking). The turn
  driver rejects positional arguments; the function itself rejects unknown, missing and
  wrong-typed keywords.
- `host_fn::<A, _>(name, signature, summary, |args: A| async move { ... })` covers most
  functions. `A` is a serde struct with `#[serde(deny_unknown_fields)]` and defaults for optional
  fields; a failed deserialization returns `HostError::BadArgs("{name}(): {serde message}")`, for
  example `search(): unknown field 'limt', expected one of 'query', 'limit', 'scrape'`. The
  closure never runs, and nothing is charged to the budget.
- `HostError::Raise` becomes a Python exception the script can catch or Claude can fix (a page
  that would not load, a rate limit). `HostError::Fatal` ends the run: the `python` tool returns
  `Err`, so `run_tools` stops, the same contract as `ClientTool` (missing or rejected API key, no
  credits, bad configuration).

### `Pack`

```rust
impl Pack {
    pub fn new(name: &'static str, summary: &'static str) -> Self;
    /// Python source run once when a session starts, after the host functions exist.
    pub fn with_source(self, source: impl Into<String>) -> Self;
    /// A Python function the source defines, listed in the tool description.
    pub fn with_python_fn(self, name: &str, signature: &str, summary: &str) -> Self;
    pub fn with_host(self, function: impl HostFunction + 'static) -> Self;
    /// Shown by help() and run by testing::check_pack.
    pub fn with_example(self, function: &str, example: Example) -> Self;
    /// Other packs that must be in the same CodeMode, by name.
    pub fn requires(self, packs: &[&'static str]) -> Self;
}
```

- A pack is host functions, Python source, or both. Built-in and app packs use this same type.
- Pack source must stay inside Monty's subset: plain functions, dicts, lists, `async def` with
  `await` and `asyncio.gather`. No class inheritance, generators, `del`, `async for`/`async with`,
  or wildcard imports.

### `CodeMode`

```rust
impl CodeMode {
    /// Validates the packs; nothing is sent and no worker is checked out yet.
    pub fn new(pool: Arc<Pool>, packs: Vec<Pack>, limits: Limits) -> Result<Self>;
    /// Ends the session and returns its worker to the pool. Dropping a CodeMode without
    /// calling this kills the worker instead: safe, but the pool must spawn a replacement.
    pub async fn finish(self) -> Result<()>;
}
impl ClientTool for CodeMode { /* name "python" */ }
```

`new` fails with `Error::validation` when two packs share a name, two functions share a name
across packs, a `requires` names a pack not given, an example names a function its pack does not
define, a function is named `help` (reserved), or the limits pass the `python` call timeout (see
`pool`). The tool definition is built
once here and cloned by `definition()`.

### Defaults for `Limits`

| Limit | Default | Enforced by |
|---|---|---|
| `max_memory` | 64 MiB | worker (`ResourceLimits::max_memory`), plus 4 MiB allocator headroom |
| `max_feed_duration` | 10 s of sandbox execution per snippet, time waiting on the host excluded | worker, pool backstop |
| `max_turn_duration` | 60 s between host round trips | worker, pool backstop |
| `max_host_calls` | 50 per conversation | this crate, before calling the function |
| `max_output_chars` | 20 000 | this crate, when building the tool result |

The host-call budget is per conversation (per `CodeMode`) because the pool's suspension count is
per session and never resets. When it runs out, every further host call raises
`RuntimeError: host call budget of 50 used up for this conversation; finish with what you have`.
The app sets a higher limit when it wants longer conversations.

`ResourceLimits::max_suspensions` stays at Monty's default of 1000. The pool counts every
suspension (a sequential `await` costs two, plus `help()`, imports and name lookups) and, past the
limit, aborts the session for good. 1000 sits far above what 50 host calls produce, so the crate's
own counter is always the limit Claude meets. If the pool's limit is ever hit anyway, the
session is discarded (see Session lifecycle).

Each host call runs under a 45 s timeout and each `python` call under a 120 s timeout, because
`max_feed_duration` excludes time spent waiting on the host. A host timeout raises
`RuntimeError: <name>() timed out after 45s`, deliberately not `TimeoutError`, which is reserved
for the sandbox's own limit and discards the session. A `python` call timeout discards the
session.

`os_policy.sleep = SleepMode::Zero`, so `time.sleep` and `asyncio.sleep` return at once instead of
reaching the host as OS calls.

## The `python` tool

### Definition

Input schema: `{"code": string}`, required.

Description, built from the packs in order:

```
Run Python (a sandboxed subset of Python 3.14) in a session that keeps variables between calls,
including ones assigned before an error.
Host functions are async and keyword-only; always await them: `hits = await search(query="...", limit=5)`.
asyncio and re are already imported. asyncio has only run, gather and sleep; gather takes no
keyword arguments, so to tolerate failures wrap each call in an async def with try/except.
The value of the last expression is returned; print() output is returned too.
No network, files or third-party packages. No class inheritance, generators or `del`.
Call `help("name")` for a function's full signature and examples.

firecrawl: web search and page scraping
  search(*, query, limit=5, scrape=False, ...) -> list[dict]: search the web
  scrape(*, url) -> dict: one page as markdown
...
```

One line per function: the signature with types removed and defaults kept, then the summary.
Host functions are listed as `name(...)`, since the header already says they are async. A Python
function a pack defines is listed as `name(...)`, or as `async name(...)` when it is an `async def`,
so Claude knows to await it.

### `help(name)`

A synchronous built-in host function. It returns a string: the full `async def` signature, the
summary, and the pack's examples for that function. An unknown name returns the list of known
names. It does not count against `max_host_calls`.

### Type checking

Off: `ReplConfig::type_check = false`, no `type_check_stubs`. Monty's checker records each
successful snippet as stubs, so a later snippet can't rebind a name to a different type
(`for k in [...]` failed this way in a real Sonnet run), and names from a snippet that raised are
invisible to it. Re-running a rejected snippet unchecked makes its state stale. See Verified items
4 and 5. Every mistake the checker caught is still caught at runtime: a misspelled function is a
`NameError`, positional and bad keyword arguments are `TypeError`s from the driver and
`HostError::BadArgs`, and a missing `await` is a `TypeError` naming `coroutine`.

### Session lifecycle

The session is a `tokio::sync::Mutex<Option<Session>>` holding one `Checkout`, the host-call
counter and a `mid_turn` flag. `run_tools` calls tools one at a time, so the lock is never
contended within a run.

1. **First call** checks out a worker with a `ReplConfig` carrying the limits and the sleep
   policy.
2. **Prelude feed.** One feed whose inputs are one `MontyObject::function(name, Some(summary))`
   per host function plus `help`, and whose code is `import asyncio`, `import re`, then each
   pack's source in pack order. The inputs
   become globals and persist for the session. A prelude failure is `Err`: a broken pack is a bug
   in the app, not something Claude can fix.
3. **Each call** sets `mid_turn`, feeds `code`, drives the turn, then clears `mid_turn`.
4. **Discard the session** (drop the checkout, which kills the worker; the next call starts again
   at step 1) when any of these happens:
   - a call starts and finds `mid_turn` set: the last call was cancelled mid-turn, for example by
     an app timeout around `run_tools`, and the checkout is stuck waiting for an answer;
   - any `PoolError` other than `Runtime` (with type checking off, the only error that keeps the
     session, per `Checkout::feed`'s docs);
   - the `python` call's 120 s timeout;
   - `Runtime` with `checkout.worker_id()` returning `None` (the memory-limit kill);
   - `Runtime` whose exception is a sandbox `TimeoutError` or `MemoryError`: Monty's README says
     the heap has no correctness guarantees after a soft limit;
   - `Runtime` whose message is the pool's `suspension limit ... exceeded`.

   The tool result for the call that triggered the discard says the session restarted and
   earlier variables are gone. For the `mid_turn` case that notice starts the next call's result,
   whatever it is, including `No sandbox worker free; try again.` The host-call counter is kept on the `CodeMode`, not the session,
   so restarting does not reset the budget.
5. **`finish`** calls `Checkout::finish` when a session exists.

### Driving a turn

```
event = feed(code)
loop:
  FunctionCall { name: "help" }       -> resume(Return(help text))
  FunctionCall { name, args, call_id } for a host function:
      args has positional values       -> resume(Error(TypeError, "keyword arguments only"))
      else                             -> record (call_id, name, args as JSON), resume(Future)
  FunctionCall { other name }         -> resume(NotFound)
  OsCall                              -> resume(NotHandled)
  NameLookup                          -> resume_name_lookup(Undefined)
  ResolveFutures { pending_call_ids } -> for each id, in order: unknown id (a call from an earlier
                                          snippet never awaited) -> RuntimeError "call it again";
                                          budget used up -> RuntimeError budget message; else
                                          charge it and run it. Run the admitted calls all at once,
                                          refund BadArgs, then resume_futures(results)
  Complete(value)                     -> done
```

- Host calls run on `ResolveFutures`, so `asyncio.gather` over 10 Jev calls runs them
  concurrently. There is no cap on concurrency: the host-call budget already bounds a `gather`,
  and running slow calls (up to 45 s each) in rounds could outlast the 120 s `python` call
  timeout, which discards the session and every result already paid for. A single `await` resolves the same way with one pending id (Verified item 2).
- The budget is charged when a call runs, not when the sandbox makes it, so an unawaited call
  costs nothing.
- If any call returns `HostError::Fatal`, stop driving at once, dropping the calls still running in
  the same `gather`, discard the session, and return that `Err`.
- `print()` goes through `on_print_sync` into a buffer that stops growing at `max_output_chars`
  and only counts the rest.

### Values for host calls

Host-call arguments and results cross between Python and JSON:

| Python | JSON | Notes |
|---|---|---|
| `None`, `bool`, `int`, `float`, `str` | same | ints outside `i64` and non-finite floats get `ValueError`, function not called |
| `list`, `tuple` | array | |
| `dict` with `str` keys | object | other key types get `TypeError`, function not called |
| anything else | | `TypeError` naming the type, function not called |

Values are a graph, so one list referenced twice is one node: `x = [x, x]` thirty times over is
small in the sandbox but 2^30 items expanded. One call's arguments may hold at most 100 000
container items in total (list and tuple elements, dict entries) and nest at most 64 deep; past
either the call raises `ValueError` (`value too large: more than 100000 items`) and the function is
not called. A reference cycle arrives as a `cycle` leaf and gets `TypeError`.

A host function's JSON result converts back the same way (objects become dicts, arrays become
lists). Written as two functions in `src/code_mode/value.rs`, walking `ObjectRef` from
`CallArgs::kwargs()` and building with `MontyObject::{none, bool, int, float, string, list,
dict}`.

### Tool result

The final value is rendered as its Python `repr()`, not converted to JSON, so sets, dates, `nan`
and int-keyed dicts all display. It is streamed through a writer that stops at 5 000 characters,
so a value with shared references never expands in full; a cut value ends with
`[result truncated: first 5000 chars shown; return less or summarize in Python]`.

```
<print output, if any>
=> <py_repr of the final value>
```

Print output and the result are capped separately so neither pushes the other out: print output
at `max_output_chars` minus 5 000, with a line `[print output truncated: N of M chars shown]`;
the `=>` value at 5 000 chars, keeping the start; an error at 5 000 chars, keeping the end, where
the exception line is.

Errors use `MontyException`'s `Display`: a CPython-style traceback with line numbers.

| Outcome | Result |
|---|---|
| `Complete` | the text above |
| `Runtime`, session kept | `is_error`, any print output plus the traceback |
| any discard from Session lifecycle step 4 | `is_error`, the cause, plus `The session restarted; earlier variables are gone.` |
| `Exhausted` | `is_error`, `No sandbox worker free; try again.` |
| `Spawn`, prelude failure, `HostError::Fatal` | `Err` |
| `Protocol`, `Finished`, `Crashed`, `Timeout`, `Disconnected`, `Shutdown` | discard (step 4); `is_error` |
| input not `{"code": string}` | `is_error` with the expected shape |

Pool failures that become `Err` use a new `Error::Sandbox { message }` variant in
`src/error.rs`, not retryable.

## Built-in packs

Constructors live in `code_mode::packs`.

### `packs::firecrawl(search: FirecrawlWebSearch, fetch: FirecrawlFetchPage, max_credits: u32)`

Built from the existing tools, not a bare `FirecrawlClient`, so the app's allowed and blocked
domains, the 10-result cap, scrape options and content cut all apply exactly as they do in the
tool loop. Each tool gets a `pub(crate)` typed entry point that runs the same checks and error
mapping as `execute` (401 and 402 are fatal) and returns results as data instead of a
`ToolResultBlock`.

`max_credits` is a per-conversation Firecrawl credit budget: a search counts its
`credits_used`, or 1 when the response reports none; a successful scrape counts 1 (its response
reports none). Once reached, both
functions raise `RuntimeError: Firecrawl credit budget of N used up; finish with what you have`.
Calls already running in one `gather` finish, so the total can overshoot by those. The call
budget alone doesn't bound credits: one `search(limit=10, scrape=True)` costs about 10.

| Function | Returns |
|---|---|
| `search(*, query: str, limit: int = 5, scrape: bool = False, include_domains: list[str] \| None = None, exclude_domains: list[str] \| None = None) -> list[dict]` | web results: `url`, `title`, `description`, `markdown` (when `scrape`), `status` |
| `scrape(*, url: str) -> dict` | `url` (final), `status`, `title`, `markdown` (cut to the tool's limit) |

Errors map as the tools already map them: `Raise` where they return `is_error`, `Fatal` where
they return `Err`.

### `packs::jev(client: JevClient)`

| Function | Returns |
|---|---|
| `ask(*, state: object, questions: list[dict]) -> dict` | answers by question name, each with its probabilities; question dicts match the `jev` tool's input |
| `yes_no(*, state: object, question: str, min_certainty: float = 0.8) -> dict` | `answer` (`bool` or `None` when unsure), `best`, `certainty` |
| `choose(*, state: object, question: str, options: list[str], min_confidence: float = 0.7) -> dict` | `answer` (`str` or `None`), `best`, `confidence`, `probabilities` |
| `rate(*, state: object, question: str, levels: list[str], min_confidence: float = 0.7) -> dict` | `answer` (`str` or `None`), `best`, `confidence`, `probabilities`; levels lowest first |

`JevClient::choose` and `rate` are generic over compile-time types, so `choose` and `rate` here go
through `JevClient::ask` with one question and read `JevResponse::choice` / `score`. The `jev`
tool's question parsing (`ToolQuestion`) and fatal-error check (`is_fatal`) become `pub(crate)`
and are reused, so errors map as `JevTool` maps them.

### `packs::research()`, requires `firecrawl`, `jev`

| Function | Does |
|---|---|
| `search_and_rank(*, query, task, limit=5, keep=3, min_confidence=0.7)` | search with scrape, `rate` each page against `task` on `["not useful", "partly useful", "directly useful"]` concurrently, return the top `keep` as dicts with `url`, `title`, `level`, `confidence`; a page whose `rate` raised `RuntimeError` or `ValueError` is `{"url", "title", "level": None, "confidence": None, "error"}` and sorts after every rated page |
| `scrape_many(*, urls)` | `scrape` concurrently, one dict per URL, failures as `{"url", "error"}` instead of raising |
| `dedupe_urls(urls)` | drop repeats ignoring scheme, host case, `www.`, trailing slash and fragment, keep order |

### `packs::triage()`, requires `jev`

| Function | Does |
|---|---|
| `classify_all(*, items, question, options, min_confidence=0.7)` | `choose` per item concurrently; returns items with `answer` and `confidence` added; an item whose `choose` raised `RuntimeError` or `ValueError` comes back with `answer` and `confidence` `None` and `"error": str(error)` |
| `score_all(*, items, question, levels, min_confidence=0.7)` | the same with `rate` |
| `group_by(rows, key)` | dict of key value to list of rows, `None` grouped under `"unsure"` |

Items are dicts or strings; a string becomes `{"text": item}`.

Per-item failures, here and in `search_and_rank`, are only `RuntimeError` and `ValueError`: a
`TypeError` from arguments every item shares still raises once instead of once per item. Rows that
succeeded have no `error` key; the summaries tell Claude to filter on it.

### `packs::reports()`

| Function | Does |
|---|---|
| `table(rows, columns=None)` | markdown table string; columns default to the first row's keys, so an `error` column appears only when the first row has one |
| `to_csv(rows, columns=None)` | CSV string through `csv`; columns default as for `table` |
| `count_by(rows, key)` | list of `{key, count}` sorted by count, descending |

Plain functions, no host calls.

## App packs

```rust
let shop = Pack::new("shop", "this store's orders and customers, read-only")
    .with_host(host_fn(
        "orders",
        "orders(*, since: str, status: str | None = None) -> list[dict]",
        "orders placed on or after an ISO date",
        move |args| { let db = db.clone(); async move { db.orders_json(args).await } },
    ))
    .with_example("orders", Example {
        code: r#"len(await orders(since="2026-10-01"))"#,
        expect: "=> 2",
    });

// once per app
let pool = code_mode::pool(monty_path, 4).await?;
// once per conversation
let code = CodeMode::new(pool.clone(), vec![packs::reports(), shop], Limits::default())?;
```

## Testing

### `testing::check_pack`

```rust
pub async fn check_pack(pool: Arc<Pool>, pack: Pack, fakes: Vec<Pack>) -> Result<()>;
```

Builds a `CodeMode` from `fakes` plus `pack`, runs every `Example` in a fresh session, and fails
naming the example when the tool result text differs from `expect`. `fakes` are packs of host
functions returning canned JSON in place of the real ones (a fake `firecrawl` pack for
`research`). App authors test their own packs the same way.

### In this crate (`src/code_mode/tests.rs`, feature `code-mode`)

In-crate like the firecrawl and jev tests, because `test_support::serve` is `pub(crate)`. The
tests need the worker: they read `MONTY_BIN` and fail with a message naming the install command
when it is unset. CI installs `monty-runtime` 1.2.0 with `--locked` and sets it.

1. Every built-in pack passes `check_pack` with fakes.
2. A `run_tools` run against the fake Messages server where Claude's `tool_use` carries a script
   calling `search` and `rate`: the fakes ran, `gather` ran them concurrently (fakes record
   overlapping start and end times), and the tool result went back in the next request.
3. Errors: `serch(...)` raises `NameError` and no host call ran; `search(query="x", limt=3)`
   raises `TypeError` naming `limt`, the function doesn't run and the budget is unchanged; a
   positional call raises `TypeError`; a raised `ValueError` from a host function is catchable in
   Python; a host function slower than its timeout raises `RuntimeError`, and the session is
   kept; `HostError::Fatal` makes `run_tools` return `Err`; the budget message appears at call 51
   for **sequential** awaits, not only gathered ones; an unawaited call from an earlier snippet,
   awaited later, raises a `RuntimeError` without a host call (Monty 1.2.0 raises its own `asyncio
   scheduler stalled` error before the driver sees the call, so the driver's "call it again" answer
   is not reached).
4. Session: a variable set in one call is readable in the next, including one assigned before an
   error, and one rebound to a different type; after a forced memory-limit error
   the next call reports a restarted session and the variable is gone; a `call()` future dropped
   while a host function runs leaves the next call working on a fresh session.
5. Validation: duplicate pack names, duplicate function names, a missing required pack, an example
   naming a function its pack does not define, and a function named `help` each fail in
   `CodeMode::new`. Input that is not `{"code": string}` gets an `is_error` result, and a pack
   whose source raises makes the call return `Err`.
6. Values: the host-call table both directions, including the `ValueError` and `TypeError` cases;
   a final value of `{1, 2}` renders as `=> {1, 2}`.
7. Firecrawl policy: a `search` with a blocked domain is refused, `limit=100` is capped at 10, a
   402 is fatal, the credit budget stops calls once reached, and `search(scrape=True)` sends the
   tool's scrape options and returns markdown cut to its limit.
8. Output: 30 000 chars of prints then a raised error returns the capped prints and the whole
   traceback with its line number.

9. Limits and discards: a sandbox `TimeoutError` from `max_feed_duration`, the `python` call
   timeout, and the pool's suspension limit (1 100 `help()` calls) each discard the session; a
   `BadArgs` inside a `gather` is refunded while the other calls run; the first `Fatal` in a
   `gather` returns without waiting for the rest; the cancelled-call notice survives a full pool;
   limits past the `python` call timeout fail in `CodeMode::new`.
10. Value sizes: a list doubled 30 times through shared references raises `ValueError` as an
    argument and renders truncated as the final value, both quickly; 100 000 items pass and
    100 001 do not; nesting past 64 levels raises `ValueError`.
11. Firecrawl policy in code mode: a blocked domain in search results is dropped, and a scrape
    redirected to a blocked domain is refused.

## Verified against the 1.2.0 worker

Run on 2026-10-21 with scratch harnesses (`/tmp/monty-measure/poolsrc/src/bin/verify.rs` and
`verify2.rs`). Items 1 to 4 used a pool with `type_check = true` and the stub
`async def double(*, x: int) -> int: ...`; they led to type checking being turned off.

1. **Feed inputs persist.** A host function passed as an input on the prelude feed is callable in
   later feeds with no inputs, and a Python function the prelude defines is visible later too.
2. **One path for single awaits and gather.** A single `await double(x=5)` arrives as
   `FunctionCall` (with `allow_eager_await = true`); answering `Future` yields `ResolveFutures [id]`
   and `resume_futures` completes it. `asyncio.gather` of three yields three `FunctionCall`s with
   `allow_eager_await = false`, then one `ResolveFutures` with all three ids. The turn driver
   above handles both unchanged.
3. **Stubs check calls before anything runs.** `async def` stubs are accepted. A positional call,
   an unknown keyword and a missing `await` (`double(x=2) + 1`) are all rejected as `Typing`
   errors with no host call made.
4. **Confirmed: names from a failed snippet are invisible to the checker.** After
   `pages = await double(x=3)` then `raise ValueError`, the next snippet `pages` fails type
   checking ("Name `pages` used when not defined"), although the value exists at runtime (the same
   snippet with type checking skipped returns 6).
5. **Type checking blocks rebinding, and there is no clean way around it.** Each committed
   snippet becomes stubs star-imported into the next, so `n = 1` then `n = 'str'` fails with
   `invalid-assignment`, as did `for k in [...]` in a real Sonnet run.
   - `# type: ignore` on the line works but depends on Claude.
   - Annotating `m: object = 1` first works but needs foresight.
   - Re-running the rejected snippet with `skip_type_check` runs it, but skipped snippets are not
     committed, so the checker goes stale: it still types `n` as `int` and rejects `n.upper()`,
     and a name defined only in a skipped snippet is `unresolved-reference` later.

   With `type_check = false` all of it works: rebinding, and a value assigned before an error is
   readable in the next snippet. Calling an undefined function arrives as a `FunctionCall` the
   driver answers with `NotFound`, which raises `NameError`. A missing `await` raises
   `TypeError: unsupported operand type(s) for +: 'coroutine' and 'int'`. Unknown and missing
   keywords reach the host, which is why `host_fn` validates them.

## Guidance for apps

From real Sonnet 5.5 runs of the PoC (`/tmp/codemode-demo`), a five-host price comparison:

- **Turn on prompt caching** (`MessageCreateParams::with_cache_control`). Without it, input grew
  to 19k tokens per turn and the run cost $0.20; with it, nearly all input was cache reads and the
  run cost $0.08.
- **Allow about 15 turns.** At 8, Sonnet ran out mid-research.
- **Put this in the system prompt:** "Do each step in one snippet: fetch everything you need at
  once with asyncio.gather, then extract in Python. Print only what you need for the answer, not
  raw pages. When you have enough, stop calling the tool and answer."
- **Check the final answer is not empty.** When `run_tools` forces `tool_choice: none` on its last
  turn, Sonnet replied with a thinking block and no text. That is a `run_tools` gap, not a
  code-mode one.
