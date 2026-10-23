//! The `jev` pack: typed judgments from Jev inside the sandbox, with an optional request cap,
//! concurrency limit and observer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;

use crate::code_mode::{Example, HostError, Pack, PyError, host_fn, value};
use crate::error::{Error, ErrorKind, Result};
use crate::jev::{self, JevClient, JevResponse, JevUsage, Outcome, Questions, ToolQuestion};

// The one question the shortcuts ask. Jev never sees names.
const QUESTION: &str = "answer";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    state: Value,
    questions: Vec<ToolQuestion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct YesNoArgs {
    state: Value,
    question: String,
    #[serde(default = "default_certainty")]
    min_certainty: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChooseArgs {
    state: Value,
    question: String,
    options: Vec<String>,
    #[serde(default = "default_confidence")]
    min_confidence: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RateArgs {
    state: Value,
    question: String,
    levels: Vec<String>,
    #[serde(default = "default_confidence")]
    min_confidence: f64,
}

fn default_certainty() -> f64 {
    0.8
}

fn default_confidence() -> f64 {
    0.7
}

/// Jev requests used by one conversation: a snapshot from [`JevBudget::used`] to save, and to
/// restore with [`JevBudget::with_used`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JevUsed {
    /// Requests sent, whether or not they succeeded. Calls refused before sending, for invalid
    /// questions or state or by the cap, are not counted.
    pub requests: u32,
}

impl JevUsed {
    /// A snapshot of this many requests.
    #[must_use]
    pub fn new(requests: u32) -> Self {
        Self { requests }
    }
}

/// A per-conversation cap on Jev requests, shared by every function of the pack.
///
/// A call of `ask`, `yes_no`, `choose` or `rate` counts as one request once it is sent, whether
/// or not it succeeds; client retries are not counted again. A call with invalid questions or
/// state is refused before sending and not counted. Once the cap is reached, the functions raise
/// `RuntimeError` without sending anything.
#[derive(Debug)]
pub struct JevBudget {
    max_requests: u32,
    requests: AtomicU32,
}

impl JevBudget {
    /// A budget of `max_requests`, none used yet.
    #[must_use]
    pub fn new(max_requests: u32) -> Self {
        Self {
            max_requests,
            requests: AtomicU32::new(0),
        }
    }

    /// Start from a saved snapshot, such as after a restart.
    #[must_use]
    pub fn with_used(self, used: JevUsed) -> Self {
        self.requests.store(used.requests, Ordering::SeqCst);
        self
    }

    /// The requests used so far.
    #[must_use]
    pub fn used(&self) -> JevUsed {
        JevUsed {
            requests: self.requests.load(Ordering::SeqCst),
        }
    }

    /// Counts one request, or gives the reason it is refused when the cap is reached.
    fn take(&self) -> std::result::Result<(), String> {
        self.requests
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                (used < self.max_requests).then(|| used + 1)
            })
            .map(|_| ())
            .map_err(|_| {
                format!(
                    "Jev request budget of {} used up; finish with what you have",
                    self.max_requests
                )
            })
    }
}

/// What the pack did for one call, in order.
///
/// A sent call gives `Asking`, then `Answered` or `Failed`. A call with invalid questions or
/// state gives only `Failed`, and one stopped by the budget only `Refused`. A call dropped by the
/// host-call timeout while its request is in flight may give `Asking` with nothing after it.
/// `function` is the Python function called: `"ask"`, `"yes_no"`, `"choose"` or `"rate"`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum JevEvent {
    /// A request is about to be sent, after the checks, the concurrency limit and the budget.
    Asking {
        /// The Python function called.
        function: &'static str,
        /// How many questions it asks.
        questions: usize,
    },
    /// Jev answered.
    Answered {
        /// The Python function called.
        function: &'static str,
        /// Tokens consumed.
        usage: JevUsage,
        /// `x-typesafe-request-id`, or `cf-ray` when that is missing.
        request_id: Option<String>,
        /// Time from sending to the answer, retries included.
        latency: Duration,
        /// Attempts made, 1 when the first succeeded.
        attempts: usize,
    },
    /// The call failed: its questions or state were invalid, or the request failed.
    Failed {
        /// The Python function called.
        function: &'static str,
        /// Why.
        error: Error,
    },
    /// The budget was used up, so nothing was sent.
    Refused {
        /// The Python function called.
        function: &'static str,
        /// The message raised to the script.
        reason: String,
    },
}

/// Receives the [`JevPack`]'s events, for logging or accounting.
pub trait JevObserver: Send + Sync {
    /// Called once per event, on the task running the host function. Keep it quick.
    fn event(&self, event: JevEvent);
}

/// The `jev` pack: `ask`, `yes_no`, `choose` and `rate` over one Jev client.
///
/// Errors map as [`JevTool`](crate::jev::JevTool) maps them: a rejected key ends the run,
/// everything else raises. Build a new pack for each conversation.
///
/// ```no_run
/// # fn build(client: topcoat_si::jev::JevClient) {
/// use std::sync::Arc;
///
/// use topcoat_si::code_mode::packs::{JevBudget, JevPack};
///
/// let budget = Arc::new(JevBudget::new(120));
/// let pack = JevPack::new(client)
///     .with_budget(budget.clone())
///     .with_max_concurrency(4)
///     .build();
/// // ... later, save `budget.used()` with the conversation ...
/// # }
/// ```
pub struct JevPack {
    client: JevClient,
    budget: Option<Arc<JevBudget>>,
    max_concurrency: Option<usize>,
    observer: Option<Arc<dyn JevObserver>>,
}

impl JevPack {
    /// A pack with no request cap, no concurrency limit and no observer.
    #[must_use]
    pub fn new(client: JevClient) -> Self {
        Self {
            client,
            budget: None,
            max_concurrency: None,
            observer: None,
        }
    }

    /// Cap the requests. Keep the `Arc` to read [`JevBudget::used`] later.
    #[must_use]
    pub fn with_budget(mut self, budget: Arc<JevBudget>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Send at most `n` requests at once (at least 1); further calls in a `gather` wait. The
    /// wait counts against the host-call timeout.
    #[must_use]
    pub fn with_max_concurrency(mut self, n: usize) -> Self {
        self.max_concurrency = Some(n.max(1));
        self
    }

    /// Send every [`JevEvent`] to `observer`.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn JevObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// The pack.
    #[must_use]
    pub fn build(self) -> Pack {
        let asker = Arc::new(Asker {
            client: self.client,
            budget: self.budget,
            permits: self.max_concurrency.map(Semaphore::new),
            observer: self.observer,
        });
        let (yes_no_asker, choose_asker, rate_asker) =
            (asker.clone(), asker.clone(), asker.clone());
        Pack::new("jev", "fast typed judgments about a state (text, dict or list)")
            .with_host(host_fn(
                "ask",
                "ask(*, state: object, questions: list[dict]) -> dict",
                "several questions in one request; questions as in the jev tool (name, type \
                 noul, choice or score, instructions, options or levels); answers by name",
                move |args: AskArgs| {
                    let asker = asker.clone();
                    async move {
                        let questions = Questions::from_tool(&args.questions);
                        let response = asker.ask("ask", args.state, &questions).await?;
                        Ok(Value::Object(jev::answers(&response, &args.questions)))
                    }
                },
            ))
            .with_host(host_fn(
                "yes_no",
                "yes_no(*, state: object, question: str, min_certainty: float = 0.8) -> dict",
                "answer is True, False or None when unsure; also best and certainty",
                move |args: YesNoArgs| {
                    let asker = yes_no_asker.clone();
                    async move {
                        let questions = Questions::new().noul(QUESTION, args.question);
                        let response = asker.ask("yes_no", args.state, &questions).await?;
                        let (outcome, certainty) =
                            mapped(response.yes_no_certainty(QUESTION, args.min_certainty))?;
                        let (Outcome::Answered(best) | Outcome::Unsure { best, .. }) = outcome;
                        Ok(json!({"answer": outcome.answered(), "best": best, "certainty": certainty}))
                    }
                },
            ))
            .with_host(host_fn(
                "choose",
                "choose(*, state: object, question: str, options: list[str], min_confidence: \
                 float = 0.7) -> dict",
                "pick one option; answer is the option or None when unsure; also best, \
                 confidence and probabilities",
                move |args: ChooseArgs| {
                    let asker = choose_asker.clone();
                    async move {
                        let questions = Questions::new().choice(
                            QUESTION,
                            args.question,
                            args.options.into_iter().map(|o| (o, None::<String>)),
                        );
                        let response = asker.ask("choose", args.state, &questions).await?;
                        value::to_json(choice(&response, args.min_confidence)?)
                    }
                },
            ))
            .with_host(host_fn(
                "rate",
                "rate(*, state: object, question: str, levels: list[str], min_confidence: float \
                 = 0.7) -> dict",
                "rate on levels, lowest first; answer is the level or None when unsure; also \
                 best, confidence and probabilities",
                move |args: RateArgs| {
                    let asker = rate_asker.clone();
                    async move {
                        let questions =
                            Questions::new().score(QUESTION, args.question, &args.levels);
                        let response = asker.ask("rate", args.state, &questions).await?;
                        value::to_json(score(&response, &args.levels, args.min_confidence)?)
                    }
                },
            ))
            .with_example(
                "yes_no",
                Example {
                    code: r#"await yes_no(state="Please refund me", question="Is this a refund request?")"#,
                    expect: "=> {'answer': True, 'best': True, 'certainty': 0.98}",
                },
            )
            .with_example(
                "choose",
                Example {
                    code: r#"(await choose(state="The app crashes", question="Which team?", options=["billing", "technical"]))["answer"]"#,
                    expect: "=> 'technical'",
                },
            )
            .with_example(
                "rate",
                Example {
                    code: r#"(await rate(state="Fix it today", question="How urgent?", levels=["low", "high"]))["answer"]"#,
                    expect: "=> 'high'",
                },
            )
            .with_example(
                "ask",
                Example {
                    code: r#"(await ask(state="Refund me", questions=[{"name": "refund", "type": "noul", "instructions": "Refund requested?"}]))["refund"]["probability_yes"]"#,
                    expect: "=> 0.99",
                },
            )
    }
}

/// `ask`, `yes_no`, `choose` and `rate` over one Jev client, with no cap, concurrency limit or
/// observer: `JevPack::new(client).build()`.
#[must_use]
pub fn jev(client: JevClient) -> Pack {
    JevPack::new(client).build()
}

/// The shared path of every function: checks, concurrency, budget, observer, then the request.
struct Asker {
    client: JevClient,
    budget: Option<Arc<JevBudget>>,
    permits: Option<Semaphore>,
    observer: Option<Arc<dyn JevObserver>>,
}

impl Asker {
    async fn ask(
        &self,
        function: &'static str,
        state: Value,
        questions: &Questions,
    ) -> std::result::Result<JevResponse, HostError> {
        let request = match self.client.prepare(state, questions) {
            Ok(request) => request,
            Err(error) => return Err(self.failed(function, error)),
        };
        // The semaphore is never closed, so `acquire` cannot fail.
        let _permit = match &self.permits {
            Some(permits) => permits.acquire().await.ok(),
            None => None,
        };
        if let Some(budget) = &self.budget
            && let Err(reason) = budget.take()
        {
            self.notify(JevEvent::Refused {
                function,
                reason: reason.clone(),
            });
            return Err(HostError::Raise {
                kind: PyError::RuntimeError,
                message: reason,
            });
        }
        self.notify(JevEvent::Asking {
            function,
            questions: questions.len(),
        });
        let start = Instant::now();
        match self.client.send(request).await {
            Ok(response) => {
                self.notify(JevEvent::Answered {
                    function,
                    usage: response.usage,
                    request_id: response.request_id.clone(),
                    latency: start.elapsed(),
                    attempts: response.attempts,
                });
                Ok(response)
            }
            Err(error) => Err(self.failed(function, error)),
        }
    }

    /// Reports a failure and maps it for the script.
    fn failed(&self, function: &'static str, error: Error) -> HostError {
        self.notify(JevEvent::Failed {
            function,
            error: error.clone(),
        });
        host_error(error)
    }

    fn notify(&self, event: JevEvent) {
        if let Some(observer) = &self.observer {
            observer.event(event);
        }
    }
}

/// A gated pick: `answer` is `None` when unsure.
#[derive(Serialize)]
struct Judgment<'label> {
    answer: Option<&'label str>,
    best: Option<&'label str>,
    confidence: f64,
    probabilities: Map<String, Value>,
}

fn choice(
    response: &JevResponse,
    min_confidence: f64,
) -> std::result::Result<Judgment<'_>, HostError> {
    let answer = mapped(response.choice(QUESTION))?;
    Ok(Judgment {
        answer: (answer.confidence >= min_confidence).then_some(answer.choice.as_str()),
        best: Some(&answer.choice),
        confidence: answer.confidence,
        probabilities: answer
            .probabilities
            .iter()
            .map(|option| (option.label.clone(), json!(option.probability)))
            .collect(),
    })
}

fn score<'a>(
    response: &'a JevResponse,
    levels: &'a [String],
    min_confidence: f64,
) -> std::result::Result<Judgment<'a>, HostError> {
    let answer = mapped(response.score(QUESTION))?;
    let best = answer
        .mode()
        .and_then(|index| levels.get(index))
        .map(String::as_str);
    Ok(Judgment {
        answer: best.filter(|_| answer.confidence >= min_confidence),
        best,
        confidence: answer.confidence,
        probabilities: levels
            .iter()
            .zip(&answer.probabilities)
            .map(|(level, p)| (level.clone(), json!(p)))
            .collect(),
    })
}

/// Maps a Jev error as the `jev` tool does: fatal ends the run, a failed check raises
/// `ValueError`, anything else `RuntimeError`.
fn mapped<T>(outcome: Result<T>) -> std::result::Result<T, HostError> {
    outcome.map_err(host_error)
}

fn host_error(error: Error) -> HostError {
    match error {
        error if jev::is_fatal(&error) => HostError::Fatal(error),
        error if matches!(error.kind(), ErrorKind::Validation { .. }) => HostError::Raise {
            kind: PyError::ValueError,
            message: format!("Invalid jev call: {error}"),
        },
        error => HostError::Raise {
            kind: PyError::RuntimeError,
            message: format!("Jev failed: {error}"),
        },
    }
}
