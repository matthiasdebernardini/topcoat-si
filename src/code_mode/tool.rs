//! The `python` client tool: one sandbox session per conversation.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use monty_pool::{
    Checkout, OnPrint, Pool, PoolError, ReplConfig, ResumeValue, TurnEvent, on_print_sync,
};
use monty_types::{
    CallArgs, ExcType, MontyException, MontyObject, NameLookupResult, OsPolicy, ResourceLimits,
    SleepMode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::limits::{HOST_CALL_TIMEOUT, LIMIT_MARGIN, PYTHON_CALL_TIMEOUT};
use super::pack::Function;
use super::{HostError, HostFunction, Limits, Pack, value};
use crate::client_tool::ClientTool;
use crate::error::{Error, ErrorKind, Result};
use crate::http;
use crate::types::{ToolParam, ToolResultBlock, ToolUnionParam, ToolUseBlock};

pub(super) const TOOL_NAME: &str = "python";
const HELP: &str = "help";
// Characters kept of the final value and of a traceback, each.
const RESULT_CHARS: usize = 5_000;
pub(super) const RESTARTED: &str = "The session restarted; earlier variables are gone.";
pub(super) const CANCELLED: &str = "The last python call was cancelled before it finished.";

const DESCRIPTION: &str = "Run Python (a sandboxed subset of Python 3.14) in a session that \
keeps variables between calls,
including ones assigned before an error.
Host functions are async and keyword-only; always await them: `hits = await search(query=\"...\", limit=5)`.
asyncio and re are already imported. asyncio has only run, gather and sleep; gather takes no
keyword arguments, so to tolerate failures wrap each call in an async def with try/except.
The value of the last expression is returned; print() output is returned too.
No network, files or third-party packages. No class inheritance, generators or `del`.
Call `help(\"name\")` for a function's full signature and examples.";

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
    pub(super) session: Mutex<Option<Session>>,
    // Per conversation: the pool's suspension count is per session and never resets.
    pub(super) host_calls: AtomicUsize,
    pub(super) host_timeout: Duration,
    pub(super) call_timeout: Duration,
}

pub(super) struct Session {
    checkout: Checkout,
    // Set while a call drives this session; still set on the next call only if that call was
    // cancelled mid-turn, leaving the checkout waiting for an answer.
    pub(super) mid_turn: bool,
}

/// What one `python` call produced, before it becomes a tool result.
pub(super) struct Reply {
    pub(super) text: String,
    pub(super) is_error: bool,
}

impl Reply {
    fn error(text: String) -> Self {
        Self {
            text,
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
struct Pending<'pack> {
    call_id: u32,
    function: &'pack dyn HostFunction,
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
    /// `max_turn_duration`, plus 5 seconds, passes the 120 second limit on one `python` call, or
    /// `max_output_chars` is under the 5,000 characters reserved for the value or traceback.
    pub fn try_new(pool: Arc<Pool>, packs: Vec<Pack>, limits: Limits) -> Result<Self> {
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
        if limits.max_output_chars < RESULT_CHARS {
            return Err(Error::validation(
                format!(
                    "max_output_chars must be at least {RESULT_CHARS}, the characters reserved \
                     for the value or traceback"
                ),
                Some("max_output_chars".to_string()),
            ));
        }
        let definition = definition(&packs);
        Ok(Self {
            pool,
            packs,
            limits,
            definition,
            session: Mutex::new(None),
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

    pub(super) async fn end_session(&self) -> Result<()> {
        match self.session.lock().await.take() {
            Some(session) => session.checkout.finish().await.map_err(|error| {
                Error::new(ErrorKind::Sandbox, "could not end the session").with_source(error)
            }),
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
    pub(super) async fn execute(&self, code: &str) -> Result<Reply> {
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

        let cap = self.limits.max_output_chars;
        let mut printed = Printed::default();
        let outcome = {
            // Scoped so the print callback's borrow of `printed` ends before it is read.
            let mut out = on_print_sync(|_, text| printed.push(text, cap));
            session.mid_turn = true;
            let outcome = self
                .drive(&mut session.checkout, code, Vec::new(), &mut out)
                .await;
            session.mid_turn = false;
            outcome
        };

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
                return Ok(Err(Reply::error(
                    "No sandbox worker free; try again.".to_string(),
                )));
            }
            Err(error @ PoolError::Spawn(_)) => {
                return Err(
                    Error::new(ErrorKind::Sandbox, "could not spawn a sandbox worker")
                        .with_source(error),
                );
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
        let functions = self.packs.iter().flat_map(|pack| &pack.functions);
        for function in functions {
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
            Outcome::Raised(exception) => Err(Error::new(
                ErrorKind::Sandbox,
                "the pack source failed to run",
            )
            .with_source(exception)),
            Outcome::Pool(error) => Err(Error::new(
                ErrorKind::Sandbox,
                "the pack source failed to run",
            )
            .with_source(error)),
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
            return ResumeValue::Error(value::exception(
                ExcType::TypeError,
                format!("{name}() takes keyword arguments only"),
            ));
        }
        match value::kwargs_to_json(args) {
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
        use futures::stream::TryStreamExt;

        let mut answers = Vec::new();
        let mut admitted = Vec::new();
        for id in ids {
            let Some(index) = pending.iter().position(|call| call.call_id == id) else {
                answers.push((
                    id,
                    ResumeValue::Error(value::exception(
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
                    ResumeValue::Error(value::exception(
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
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                (used < self.limits.max_host_calls).then_some(used + 1)
            })
            .is_ok()
    }

    async fn run_host(&self, call: Pending<'_>) -> Result<(u32, ResumeValue)> {
        let name = call.function.name();
        let answer =
            match tokio::time::timeout(self.host_timeout, call.function.call(call.args)).await {
                Ok(Ok(value)) => Ok(ResumeValue::Return(value::json_to_object(&value))),
                Ok(Err(HostError::BadArgs(message))) => {
                    self.host_calls.fetch_sub(1, Ordering::SeqCst);
                    Ok(ResumeValue::Error(value::exception(
                        ExcType::TypeError,
                        message,
                    )))
                }
                Ok(Err(HostError::Raise { kind, message })) => Ok(ResumeValue::Error(
                    value::exception(kind.exc_type(), message),
                )),
                Ok(Err(HostError::Fatal(error))) => Err(error),
                Err(_) => Ok(ResumeValue::Error(value::exception(
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
                http::truncate_chars(&printed.text, print_cap, ""),
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
    match value::capped_repr(value, RESULT_CHARS) {
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
            let reply = match Input::deserialize(&tool_use.input) {
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

pub(super) fn definition(packs: &[Pack]) -> ToolUnionParam {
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
