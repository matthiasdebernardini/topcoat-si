//! Code mode: Claude writes Python, a Monty sandbox runs it, and the host functions it calls are
//! answered by Rust.
//!
//! [`CodeMode`] is one client tool, `python`, for [`Anthropic::run_tools`](crate::Anthropic::run_tools).
//! The functions a script can call come in [`Pack`]s: the built-in ones in [`packs`] and any the
//! app adds with [`host_fn`] and Python source. One snippet can search, scrape, classify, filter
//! and report without the intermediate data entering Claude's context.
//!
//! Workers are `monty` processes from `monty-runtime` 1.2, installed with
//! `cargo install monty-runtime --version 1.2.0 --locked`. Create one [`pool`] per app and one
//! [`CodeMode`] per conversation.
//!
//! ```no_run
//! # async fn run(monty_path: std::path::PathBuf) -> topcoat_si::Result<()> {
//! use topcoat_si::code_mode::{self, CodeMode, Limits, packs};
//!
//! // once per app
//! let pool = code_mode::pool(monty_path, 4).await?;
//! // once per conversation
//! let code = CodeMode::new(pool.clone(), vec![packs::reports()], Limits::default())?;
//! // ... pass `&code` to run_tools ...
//! code.finish().await?;
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, TryStreamExt};
use monty_pool::{
    Checkout, OnPrint, PoolConfig, PoolError, ReplConfig, ResumeValue, TurnEvent, on_print_sync,
};
use monty_types::{
    CallArgs, ExcType, MontyException, MontyObject, NameLookupResult, OsPolicy, ResourceLimits,
    SleepMode,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::tool::ClientTool;
use crate::types::{ToolParam, ToolResultBlock, ToolUnionParam, ToolUseBlock};

pub mod packs;
pub mod testing;
mod value;

#[cfg(test)]
mod tests;

pub use monty_pool::Pool;

use value::{capped_repr, json_to_object, kwargs_to_json, raise};

const TOOL_NAME: &str = "python";
const HELP: &str = "help";
const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(10);
// The pool's backstops kill a worker this long past a sandbox time limit at the latest.
const LIMIT_MARGIN: Duration = Duration::from_secs(5);
// `max_feed_duration` excludes time waiting on the host, so these bound the host side.
const HOST_CALL_TIMEOUT: Duration = Duration::from_secs(45);
const PYTHON_CALL_TIMEOUT: Duration = Duration::from_secs(120);
// Characters kept of the final value and of a traceback, each.
const RESULT_CHARS: usize = 5_000;
const RESTARTED: &str = "The session restarted; earlier variables are gone.";
const CANCELLED: &str = "The last python call was cancelled before it finished.";

const DESCRIPTION: &str = "Run Python (a sandboxed subset of Python 3.14) in a session that \
keeps variables between calls,
including ones assigned before an error.
Host functions are async and keyword-only; always await them: `hits = await search(query=\"...\", limit=5)`.
asyncio and re are already imported. asyncio has only run, gather and sleep; gather takes no
keyword arguments, so to tolerate failures wrap each call in an async def with try/except.
The value of the last expression is returned; print() output is returned too.
No network, files or third-party packages. No class inheritance, generators or `del`.
Call `help(\"name\")` for a function's full signature and examples.";

/// Creates the shared worker pool. `monty` is the path to the monty-runtime 1.2 binary.
///
/// One pool serves every conversation in the app, so `max_workers` caps the live sandboxes. A
/// conversation that finds every worker busy waits up to 10 seconds, then gets a "no sandbox
/// free" tool result. The pool kills a worker that leaves any one request unanswered for 120
/// seconds; the sandbox's own time limits stop a snippet well before that.
pub async fn pool(monty: impl Into<PathBuf>, max_workers: usize) -> Result<Arc<Pool>> {
    if max_workers == 0 {
        return Err(Error::validation(
            "max_workers must be at least 1",
            Some("max_workers".to_string()),
        ));
    }
    let mut config = PoolConfig::subprocess(monty);
    config.min_processes = 1;
    config.max_processes = max_workers;
    config.checkout_timeout = Some(CHECKOUT_TIMEOUT);
    config.request_timeout = Some(PYTHON_CALL_TIMEOUT);
    Pool::new(config)
        .await
        .map(Arc::new)
        .map_err(|error| Error::sandbox(format!("could not start the sandbox pool: {error}")))
}

/// Limits on one conversation's sandbox.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Bytes of memory the sandbox may use. Default 64 MiB.
    pub max_memory: usize,
    /// Sandbox execution time per snippet, time waiting on the host excluded. Default 10 s.
    pub max_feed_duration: Duration,
    /// Sandbox execution time between two host round trips. Default 60 s.
    pub max_turn_duration: Duration,
    /// Host calls for the whole conversation, across session restarts. Default 50.
    pub max_host_calls: usize,
    /// Characters of one tool result: print output gets this minus 5,000, the final value or
    /// traceback 5,000. Default 20,000.
    pub max_output_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_memory: 64 * 1024 * 1024,
            max_feed_duration: Duration::from_secs(10),
            max_turn_duration: Duration::from_secs(60),
            max_host_calls: 50,
            max_output_chars: 20_000,
        }
    }
}

/// A Python exception a host function can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyError {
    /// `ValueError`.
    ValueError,
    /// `TypeError`.
    TypeError,
    /// `KeyError`.
    KeyError,
    /// `LookupError`.
    LookupError,
    /// `RuntimeError`.
    RuntimeError,
}

impl PyError {
    fn exc_type(self) -> ExcType {
        match self {
            Self::ValueError => ExcType::ValueError,
            Self::TypeError => ExcType::TypeError,
            Self::KeyError => ExcType::KeyError,
            Self::LookupError => ExcType::LookupError,
            Self::RuntimeError => ExcType::RuntimeError,
        }
    }
}

/// Why a host function failed.
#[derive(Debug, Clone)]
pub enum HostError {
    /// The arguments don't fit the function: Python `TypeError`, not charged to the budget.
    BadArgs(String),
    /// A problem Claude or the script can handle: becomes this Python exception.
    Raise {
        /// The exception type.
        kind: PyError,
        /// The exception message.
        message: String,
    },
    /// A problem nobody in the sandbox can fix: ends the run.
    Fatal(Error),
}

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
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, std::result::Result<Value, HostError>>;
}

/// A [`HostFunction`] whose keyword arguments deserialize into `A`.
///
/// `A` is a serde struct with `#[serde(deny_unknown_fields)]` and defaults for optional fields.
/// Arguments that don't deserialize raise `TypeError` naming the function, without running
/// `function` or charging the host-call budget.
pub fn host_fn<A, F, Fut>(
    name: impl Into<String>,
    signature: impl Into<String>,
    summary: impl Into<String>,
    function: F,
) -> impl HostFunction
where
    A: DeserializeOwned + Send + 'static,
    F: Fn(A) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<Value, HostError>> + Send + 'static,
{
    TypedHostFn {
        name: name.into(),
        signature: signature.into(),
        summary: summary.into(),
        function,
        args: PhantomData,
    }
}

struct TypedHostFn<A, F> {
    name: String,
    signature: String,
    summary: String,
    function: F,
    args: PhantomData<fn(A)>,
}

impl<A, F, Fut> HostFunction for TypedHostFn<A, F>
where
    A: DeserializeOwned + Send + 'static,
    F: Fn(A) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<Value, HostError>> + Send + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> &str {
        &self.signature
    }

    fn summary(&self) -> &str {
        &self.summary
    }

    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, std::result::Result<Value, HostError>> {
        match serde_json::from_value::<A>(args) {
            Ok(args) => Box::pin((self.function)(args)),
            Err(error) => {
                let message = format!("{}(): {error}", self.name);
                Box::pin(async move { Err(HostError::BadArgs(message)) })
            }
        }
    }
}

/// One example call: `code` run in a fresh session must produce exactly `expect`, the text of
/// the tool result.
#[derive(Debug, Clone, Copy)]
pub struct Example {
    /// The snippet.
    pub code: &'static str,
    /// The tool result text, such as `=> 2`.
    pub expect: &'static str,
}

enum Function {
    Host(Box<dyn HostFunction>),
    Python {
        name: String,
        signature: String,
        summary: String,
    },
}

impl Function {
    fn name(&self) -> &str {
        match self {
            Self::Host(function) => function.name(),
            Self::Python { name, .. } => name,
        }
    }

    fn summary(&self) -> &str {
        match self {
            Self::Host(function) => function.summary(),
            Self::Python { summary, .. } => summary,
        }
    }

    /// The full `def` line, without the colon.
    fn header(&self) -> String {
        match self {
            Self::Host(function) => format!("async def {}", function.signature()),
            Self::Python { signature, .. } => signature.clone(),
        }
    }

    /// The description line: the signature with types removed and defaults kept.
    fn listing(&self) -> String {
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
    name: &'static str,
    summary: &'static str,
    source: String,
    functions: Vec<Function>,
    examples: Vec<(String, Example)>,
    requires: Vec<&'static str>,
}

impl Pack {
    /// An empty pack; `summary` heads its functions in the tool description.
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
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// A Python function the source defines, listed in the tool description. `signature` is
    /// its whole `def` or `async def` line without the colon:
    /// `async def scrape_many(*, urls: list[str]) -> list[dict]`.
    pub fn with_python_fn(mut self, name: &str, signature: &str, summary: &str) -> Self {
        self.functions.push(Function::Python {
            name: name.to_string(),
            signature: signature.to_string(),
            summary: summary.to_string(),
        });
        self
    }

    /// A host function.
    pub fn with_host(mut self, function: impl HostFunction + 'static) -> Self {
        self.functions.push(Function::Host(Box::new(function)));
        self
    }

    /// Shown by help() and run by testing::check_pack.
    pub fn with_example(mut self, function: &str, example: Example) -> Self {
        self.examples.push((function.to_string(), example));
        self
    }

    /// Other packs that must be in the same CodeMode, by name.
    pub fn requires(mut self, packs: &[&'static str]) -> Self {
        self.requires.extend_from_slice(packs);
        self
    }
}

/// The `python` client tool: one sandbox session for one conversation.
///
/// The session keeps Python variables between calls, so a `CodeMode` belongs to exactly one
/// conversation. Sharing one across conversations would let one user's data reach another's
/// Claude. Workers come from the shared [`Pool`]; the session takes one on the first call.
///
/// A call answers `is_error` for anything Claude can fix: a Python exception, a full pool, a
/// restarted session. It returns `Err`, ending `run_tools`, for a worker that cannot spawn, a
/// pack whose source fails, and a host function's [`HostError::Fatal`].
pub struct CodeMode {
    pool: Arc<Pool>,
    packs: Vec<Pack>,
    limits: Limits,
    definition: ToolUnionParam,
    session: tokio::sync::Mutex<Option<Session>>,
    // Per conversation: the pool's suspension count is per session and never resets.
    host_calls: AtomicUsize,
    host_timeout: Duration,
    call_timeout: Duration,
}

struct Session {
    checkout: Checkout,
    // Set while a call drives this session; still set on the next call only if that call was
    // cancelled mid-turn, leaving the checkout waiting for an answer.
    mid_turn: bool,
}

/// What one `python` call produced, before it becomes a tool result.
struct Reply {
    text: String,
    is_error: bool,
}

impl Reply {
    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// How a fed snippet ended, when no host function failed fatally.
enum Outcome {
    Complete(MontyObject),
    Raised(MontyException),
    Pool(PoolError),
}

/// A host call the sandbox made and has not had answered yet.
struct Pending<'a> {
    call_id: u32,
    function: &'a dyn HostFunction,
    args: Value,
}

/// Print output of one feed: kept up to a cap, the rest only counted.
#[derive(Default)]
struct Printed {
    text: String,
    kept: usize,
    total: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    code: String,
}

impl CodeMode {
    /// Validates the packs; nothing is sent and no worker is checked out yet.
    ///
    /// Fails when two packs or two functions across packs share a name, a pack requires one not
    /// given or has an example for a function it does not define, a function is named `help`,
    /// which is reserved, or the shorter of `max_feed_duration` and
    /// `max_turn_duration`, plus 5 seconds, passes the 120 second limit on one `python` call.
    pub fn new(pool: Arc<Pool>, packs: Vec<Pack>, limits: Limits) -> Result<Self> {
        let invalid = |message: String| Error::validation(message, Some("packs".to_string()));
        let names: Vec<&str> = packs
            .iter()
            .flat_map(|pack| pack.functions.iter().map(Function::name))
            .collect();
        for (index, name) in names.iter().enumerate() {
            if *name == HELP {
                return Err(invalid(
                    "a function is named help, which is reserved".to_string(),
                ));
            }
            if names[..index].contains(name) {
                return Err(invalid(format!("two functions are named {name}")));
            }
        }
        for (index, pack) in packs.iter().enumerate() {
            if packs[..index]
                .iter()
                .any(|earlier| earlier.name == pack.name)
            {
                return Err(invalid(format!("two packs are named {}", pack.name)));
            }
            if let Some((function, _)) = pack
                .examples
                .iter()
                .find(|(function, _)| !pack.functions.iter().any(|f| f.name() == function))
            {
                return Err(invalid(format!(
                    "pack {} has an example for {function}, which it does not define",
                    pack.name
                )));
            }
            if let Some(missing) = pack
                .requires
                .iter()
                .find(|required| !packs.iter().any(|p| p.name == **required))
            {
                return Err(invalid(format!(
                    "pack {} requires pack {missing}, which is not given",
                    pack.name
                )));
            }
        }
        let sandbox_limit = limits.max_feed_duration.min(limits.max_turn_duration);
        if sandbox_limit + LIMIT_MARGIN > PYTHON_CALL_TIMEOUT {
            return Err(Error::validation(
                format!(
                    "max_feed_duration and max_turn_duration must leave one of them under \
                     {:?}, so the sandbox stops a snippet before the {PYTHON_CALL_TIMEOUT:?} \
                     python call timeout",
                    PYTHON_CALL_TIMEOUT - LIMIT_MARGIN
                ),
                Some("limits".to_string()),
            ));
        }
        let definition = definition(&packs);
        Ok(Self {
            pool,
            packs,
            limits,
            definition,
            session: tokio::sync::Mutex::new(None),
            host_calls: AtomicUsize::new(0),
            host_timeout: HOST_CALL_TIMEOUT,
            call_timeout: PYTHON_CALL_TIMEOUT,
        })
    }

    /// Ends the session and returns its worker to the pool. Dropping a CodeMode without
    /// calling this kills the worker instead: safe, but the pool must spawn a replacement.
    pub async fn finish(self) -> Result<()> {
        self.end_session().await
    }

    async fn end_session(&self) -> Result<()> {
        match self.session.lock().await.take() {
            Some(session) => session
                .checkout
                .finish()
                .await
                .map_err(|error| Error::sandbox(format!("could not end the session: {error}"))),
            None => Ok(()),
        }
    }

    fn host(&self, name: &str) -> Option<&dyn HostFunction> {
        self.packs
            .iter()
            .flat_map(|pack| &pack.functions)
            .find_map(|function| match function {
                Function::Host(host) if host.name() == name => Some(host.as_ref()),
                _ => None,
            })
    }

    /// Runs one snippet under the call timeout. `Err` only for failures Claude cannot fix.
    async fn execute(&self, code: &str) -> Result<Reply> {
        match tokio::time::timeout(self.call_timeout, self.run(code)).await {
            Ok(reply) => reply,
            Err(_) => {
                // The cancelled run left the session mid-turn; drop it, killing the worker.
                self.session.lock().await.take();
                Ok(Reply::error(format!(
                    "The python call ran longer than {:?} and was stopped.\n{RESTARTED}",
                    self.call_timeout
                )))
            }
        }
    }

    async fn run(&self, code: &str) -> Result<Reply> {
        let mut guard = self.session.lock().await;
        let cancelled = guard.as_ref().is_some_and(|session| session.mid_turn);
        if cancelled {
            *guard = None;
        }
        let notice = |reply: Reply| {
            if !cancelled {
                return reply;
            }
            Reply {
                text: format!("{CANCELLED} {RESTARTED}\n{}", reply.text),
                is_error: reply.is_error,
            }
        };
        let session = match guard.as_mut() {
            Some(session) => session,
            None => match self.start_session().await? {
                Ok(session) => guard.insert(session),
                Err(reply) => return Ok(notice(reply)),
            },
        };

        let printed = Arc::new(StdMutex::new(Printed::default()));
        let sink = printed.clone();
        let cap = self.limits.max_output_chars;
        let mut out = on_print_sync(move |_, text| {
            if let Ok(mut printed) = sink.lock() {
                printed.push(text, cap);
            }
        });
        session.mid_turn = true;
        let outcome = self
            .drive(&mut session.checkout, code, Vec::new(), &mut out)
            .await;
        session.mid_turn = false;
        let printed = printed
            .lock()
            .map(|mut p| std::mem::take(&mut *p))
            .unwrap_or_default();

        let (text, is_error, discard) = match outcome {
            Err(error) => {
                *guard = None;
                return Err(error);
            }
            Ok(Outcome::Complete(value)) => {
                (self.compose(&printed, &value_text(&value)), false, false)
            }
            Ok(Outcome::Raised(exception)) => {
                let discard = session.checkout.worker_id().is_none() || is_fatal(&exception);
                (
                    self.compose(&printed, &traceback_text(&exception.to_string())),
                    true,
                    discard,
                )
            }
            Ok(Outcome::Pool(error)) => (
                self.compose(
                    &printed,
                    &traceback_text(&format!("Sandbox error: {error}")),
                ),
                true,
                true,
            ),
        };
        let text = if discard {
            *guard = None;
            format!("{text}\n{RESTARTED}")
        } else {
            text
        };
        Ok(notice(Reply { text, is_error }))
    }

    /// Checks out a worker and runs the prelude. The inner `Err` is a reply for Claude when no
    /// worker could be had; the outer one a failure it cannot fix.
    async fn start_session(&self) -> Result<std::result::Result<Session, Reply>> {
        let repl = ReplConfig {
            limits: Some(ResourceLimits {
                max_memory: Some(self.limits.max_memory),
                max_feed_duration: Some(self.limits.max_feed_duration),
                max_turn_duration: Some(self.limits.max_turn_duration),
                ..ResourceLimits::default()
            }),
            os_policy: OsPolicy {
                sleep: SleepMode::Zero,
                ..OsPolicy::default()
            },
            ..ReplConfig::default()
        };
        let mut checkout = match self.pool.checkout(&repl).await {
            Ok(checkout) => checkout,
            Err(PoolError::Exhausted) => {
                return Ok(Err(Reply::error("No sandbox worker free; try again.")));
            }
            Err(PoolError::Spawn(message)) => {
                return Err(Error::sandbox(format!(
                    "could not spawn a sandbox worker: {message}"
                )));
            }
            Err(error) => {
                return Ok(Err(Reply::error(format!(
                    "The sandbox failed to start ({error}); try again."
                ))));
            }
        };

        let mut inputs = vec![(
            HELP.to_string(),
            MontyObject::function(
                HELP,
                Some("A function's signature and examples".to_string()),
            ),
        )];
        for function in self.packs.iter().flat_map(|pack| &pack.functions) {
            if let Function::Host(host) = function {
                inputs.push((
                    host.name().to_string(),
                    MontyObject::function(host.name(), Some(host.summary().to_string())),
                ));
            }
        }
        let mut prelude = "import asyncio\nimport re\n".to_string();
        for pack in &self.packs {
            prelude.push_str(&pack.source);
            prelude.push('\n');
        }
        let mut out = on_print_sync(|_, _| {});
        match self
            .drive(&mut checkout, &prelude, inputs, &mut out)
            .await?
        {
            Outcome::Complete(_) => Ok(Ok(Session {
                checkout,
                mid_turn: false,
            })),
            Outcome::Raised(exception) => Err(Error::sandbox(format!(
                "the pack source failed to run:\n{exception}"
            ))),
            Outcome::Pool(error) => Err(Error::sandbox(format!(
                "the pack source failed to run: {error}"
            ))),
        }
    }

    /// Feeds `code` and answers the sandbox until the snippet completes or fails. `Err` is a
    /// host function's fatal error; the caller must discard the session.
    async fn drive(
        &self,
        checkout: &mut Checkout,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        out: OnPrint<'_>,
    ) -> Result<Outcome> {
        let mut pending: Vec<Pending<'_>> = Vec::new();
        // Type checking is off for the session, so there is nothing to skip.
        let skip_type_check = false;
        let mut event = checkout
            .feed(code, inputs, Vec::new(), skip_type_check, &mut *out)
            .await;
        loop {
            let turn = match event {
                Ok(turn) => turn,
                Err(PoolError::Runtime(exception)) => return Ok(Outcome::Raised(exception)),
                Err(error) => return Ok(Outcome::Pool(error)),
            };
            event = match turn {
                TurnEvent::Complete(value) => return Ok(Outcome::Complete(value)),
                TurnEvent::FunctionCall {
                    function_name,
                    args,
                    call_id,
                    ..
                } => {
                    let answer = self.answer_call(&function_name, &args, call_id, &mut pending);
                    checkout.resume(answer, &mut *out).await
                }
                TurnEvent::OsCall { .. } => {
                    checkout.resume(ResumeValue::NotHandled, &mut *out).await
                }
                TurnEvent::NameLookup { .. } => {
                    checkout
                        .resume_name_lookup(NameLookupResult::Undefined, &mut *out)
                        .await
                }
                TurnEvent::ResolveFutures {
                    pending_call_ids, ..
                } => {
                    let answers = self.resolve(pending_call_ids, &mut pending).await?;
                    checkout.resume_futures(answers, &mut *out).await
                }
            };
        }
    }

    /// Answers a function call at once: `help` returns its text, a host call becomes a future
    /// resolved later, anything else is not found.
    fn answer_call<'a>(
        &'a self,
        name: &str,
        args: &CallArgs,
        call_id: u32,
        pending: &mut Vec<Pending<'a>>,
    ) -> ResumeValue {
        if name == HELP {
            let topic = args
                .arg(0)
                .or_else(|| args.kwarg("name"))
                .and_then(|value| value.as_str().map(str::to_string));
            return ResumeValue::Return(MontyObject::string(self.help(topic.as_deref())));
        }
        let Some(function) = self.host(name) else {
            return ResumeValue::NotFound;
        };
        if args.arg(0).is_some() {
            return ResumeValue::Error(raise(
                ExcType::TypeError,
                format!("{name}() takes keyword arguments only"),
            ));
        }
        match kwargs_to_json(args) {
            Ok(args) => {
                pending.push(Pending {
                    call_id,
                    function,
                    args,
                });
                ResumeValue::Future
            }
            Err(exception) => ResumeValue::Error(exception),
        }
    }

    /// Runs the host calls the sandbox awaits, charging each to the budget as it starts.
    async fn resolve(
        &self,
        ids: Vec<u32>,
        pending: &mut Vec<Pending<'_>>,
    ) -> Result<Vec<(u32, ResumeValue)>> {
        let mut answers = Vec::new();
        let mut admitted = Vec::new();
        for id in ids {
            let Some(index) = pending.iter().position(|call| call.call_id == id) else {
                answers.push((
                    id,
                    ResumeValue::Error(raise(
                        ExcType::RuntimeError,
                        "this call came from an earlier snippet and was never awaited; call \
                         it again"
                            .to_string(),
                    )),
                ));
                continue;
            };
            let call = pending.swap_remove(index);
            if self.charge() {
                admitted.push(call);
            } else {
                answers.push((
                    id,
                    ResumeValue::Error(raise(
                        ExcType::RuntimeError,
                        format!(
                            "host call budget of {} used up for this conversation; finish with \
                             what you have",
                            self.limits.max_host_calls
                        ),
                    )),
                ));
            }
        }
        // All at once: the budget bounds how many, and running them in rounds could outlast
        // the python call timeout. The first fatal error returns at once, dropping the rest.
        let running: FuturesUnordered<_> = admitted
            .into_iter()
            .map(|call| self.run_host(call))
            .collect();
        let resolved: Vec<(u32, ResumeValue)> = running.try_collect().await?;
        answers.extend(resolved);
        Ok(answers)
    }

    /// Takes one host call from the budget, or reports it used up.
    fn charge(&self) -> bool {
        self.host_calls
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                (used < self.limits.max_host_calls).then_some(used + 1)
            })
            .is_ok()
    }

    async fn run_host(&self, call: Pending<'_>) -> Result<(u32, ResumeValue)> {
        let name = call.function.name();
        let answer =
            match tokio::time::timeout(self.host_timeout, call.function.call(call.args)).await {
                Ok(Ok(value)) => Ok(ResumeValue::Return(json_to_object(&value))),
                Ok(Err(HostError::BadArgs(message))) => {
                    self.host_calls.fetch_sub(1, Ordering::SeqCst);
                    Ok(ResumeValue::Error(raise(ExcType::TypeError, message)))
                }
                Ok(Err(HostError::Raise { kind, message })) => {
                    Ok(ResumeValue::Error(raise(kind.exc_type(), message)))
                }
                Ok(Err(HostError::Fatal(error))) => Err(error),
                Err(_) => Ok(ResumeValue::Error(raise(
                    ExcType::RuntimeError,
                    format!("{name}() timed out after {:?}", self.host_timeout),
                ))),
            };
        answer.map(|answer| (call.call_id, answer))
    }

    /// `help(name)`: the full signature, summary and examples, or the known names.
    fn help(&self, topic: Option<&str>) -> String {
        let functions = || self.packs.iter().flat_map(|pack| &pack.functions);
        let Some(function) = topic.and_then(|topic| functions().find(|f| f.name() == topic)) else {
            let names: Vec<&str> = functions().map(Function::name).collect();
            return format!(
                "Unknown name {:?}. Functions: {}",
                topic.unwrap_or_default(),
                names.join(", ")
            );
        };
        let examples: String = self
            .packs
            .iter()
            .flat_map(|pack| &pack.examples)
            .filter(|(name, _)| name == function.name())
            .map(|(_, example)| format!("\n\nExample:\n{}\n{}", example.code, example.expect))
            .collect();
        format!("{}\n{}{examples}", function.header(), function.summary())
    }

    /// Print output capped at `max_output_chars` minus 5,000, then `tail`.
    fn compose(&self, printed: &Printed, tail: &str) -> String {
        let print_cap = self.limits.max_output_chars.saturating_sub(RESULT_CHARS);
        let prints = if printed.total > print_cap {
            format!(
                "{}\n[print output truncated: {print_cap} of {} chars shown]\n",
                take_chars(&printed.text, print_cap),
                printed.total
            )
        } else if printed.text.is_empty() || printed.text.ends_with('\n') {
            printed.text.clone()
        } else {
            format!("{}\n", printed.text)
        };
        prints + tail
    }
}

/// `=> ` and the value's repr, keeping its first 5,000 characters.
fn value_text(value: &MontyObject) -> String {
    match capped_repr(value, RESULT_CHARS) {
        (repr, false) => format!("=> {repr}"),
        (repr, true) => format!(
            "=> {repr}\n[result truncated: first {RESULT_CHARS} chars shown; return less or \
             summarize in Python]"
        ),
    }
}

/// A traceback, keeping its last 5,000 characters, where the exception line is.
fn traceback_text(text: &str) -> String {
    let length = text.chars().count();
    if length <= RESULT_CHARS {
        return text.to_string();
    }
    format!(
        "[traceback truncated: last {RESULT_CHARS} of {length} chars shown]\n{}",
        text.chars().skip(length - RESULT_CHARS).collect::<String>()
    )
}

impl ClientTool for CodeMode {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn definition(&self) -> ToolUnionParam {
        self.definition.clone()
    }

    fn call<'a>(&'a self, tool_use: &'a ToolUseBlock) -> BoxFuture<'a, Result<ToolResultBlock>> {
        Box::pin(async move {
            let reply = match serde_json::from_value::<Input>(tool_use.input.clone()) {
                Ok(input) => self.execute(&input.code).await?,
                Err(error) => Reply::error(format!(
                    "Invalid python input: {error}. Send an object with only a \"code\" string."
                )),
            };
            Ok(ToolResultBlock::new(tool_use.id.clone())
                .with_string_content(reply.text)
                .with_error(reply.is_error))
        })
    }
}

impl Printed {
    fn push(&mut self, text: &str, cap: usize) {
        let length = text.chars().count();
        self.total += length;
        let room = cap.saturating_sub(self.kept).min(length);
        if room > 0 {
            self.text.extend(text.chars().take(room));
            self.kept += room;
        }
    }
}

/// Exceptions after which the session cannot be trusted: Monty gives no heap guarantees past a
/// soft limit, and the pool aborts a session past its suspension limit for good.
fn is_fatal(exception: &MontyException) -> bool {
    let suspension_limit = exception
        .message()
        .is_some_and(|m| m.starts_with("suspension limit") && m.ends_with("exceeded"));
    let soft_limit = matches!(
        exception.exc_type(),
        ExcType::TimeoutError | ExcType::MemoryError
    );
    soft_limit || suspension_limit
}

fn definition(packs: &[Pack]) -> ToolUnionParam {
    let listing: String = packs
        .iter()
        .map(|pack| {
            let functions: String = pack
                .functions
                .iter()
                .map(|function| format!("\n  {}: {}", function.listing(), function.summary()))
                .collect();
            format!("\n\n{}: {}{functions}", pack.name, pack.summary)
        })
        .collect();
    let description = format!("{DESCRIPTION}{listing}");
    let schema = json!({
        "type": "object",
        "properties": {
            "code": {"type": "string", "description": "Python source to run."}
        },
        "required": ["code"],
        "additionalProperties": false
    });
    ToolUnionParam::CustomTool(
        ToolParam::new(TOOL_NAME.to_string(), schema).with_description(description),
    )
}

/// `name(*, a: int = 1, b: list[str]) -> T` becomes `name(*, a=1, b) -> T`.
fn untyped(signature: &str) -> String {
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
            let name = split_top_level(assignment[0], ':')[0];
            let default = assignment.get(1).map(|d| d.trim());
            default.map_or_else(
                || name.trim().to_string(),
                |default| format!("{}={default}", name.trim()),
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

fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut quote = None;
    for (index, c) in text.char_indices().skip_while(|(i, _)| *i < open) {
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
fn split_top_level(text: &str, separator: char) -> Vec<&str> {
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

fn take_chars(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}
