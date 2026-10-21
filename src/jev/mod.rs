//! Typed judgments from TypeSafe AI's Jev decision model.
//!
//! [`JevClient::ask`] sends one state and a set of named [`Questions`] to `POST /v1/systemone`
//! and returns every answer in a [`JevResponse`]. A question is a noul (yes or no), a choice
//! (pick one label) or a score (rate on an ordered scale). [`JevTool`] exposes the client to
//! Claude as a client-side `jev` tool.
//!
//! Probabilities come back rounded to hundredths, so they need not sum to 1 and the reported
//! choice can trail another label by 0.01. This client never rejects an answer over that.
//! Confidence measures how concentrated an answer is, not whether it is correct.
//!
//! ```no_run
//! # async fn run() -> topcoat_si::Result<()> {
//! use topcoat_si::jev::{JevClient, Outcome, Questions};
//!
//! let jev = JevClient::try_new("ts-key", "jev-1.13.0")?;
//! let questions = Questions::new()
//!     .noul("refund", "Does the customer ask for a refund?")
//!     .choice(
//!         "route",
//!         "Which team should handle the message?",
//!         [("billing", Some("Charges and refunds")), ("other", None)],
//!     );
//! let response = jev
//!     .ask("I was charged twice, please refund one.", &questions)
//!     .await?;
//! if let Outcome::Answered(true) = response.yes_no("refund", 0.8)? {
//!     println!("route to {}", response.choice("route")?.choice);
//! }
//! # Ok(())
//! # }
//! ```

mod answer;
mod choices;
mod client;
mod error_code;
mod outcome;
mod questions;
mod response;
mod tool;

#[cfg(test)]
mod tests;

pub use answer::{Answer, AnswerKind, ChoiceAnswer, LabelProbability, ScoreAnswer};
pub use choices::{ChoiceOption, Choices, Level, Levels};
pub use client::JevClient;
pub use error_code::JevErrorCode;
pub use outcome::Outcome;
pub use questions::Questions;
pub use response::{JevResponse, JevUsage, NamedAnswer};
pub use tool::JevTool;
#[cfg(feature = "code-mode")]
pub(crate) use tool::{ToolQuestion, answers, is_fatal};
