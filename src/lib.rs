#![deny(missing_docs)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

//! Topcoat SI: A typed Rust client for the Anthropic API.
//!
//! This crate provides typed access to the Messages API (buffered and streaming), tool
//! definitions, message batches, files, models, and admin usage and cost reports, with
//! per-response pricing, rate-limit capture, and tracing. Web search is a client-side tool backed
//! by Firecrawl, see [`firecrawl`].
//!
//! [`Anthropic::run_tools`] runs Claude with your [`ClientTool`]s (Firecrawl search and page fetch,
//! [`jev::JevTool`], or your own) until it answers. [`jev`] is a client for TypeSafe's Jev decision
//! model, and [`Anthropic::send_parsed`] returns replies as your own Rust types.

mod accumulating_stream;
mod anthropic;
mod backoff;
mod base_url;
mod batch_wait;
mod client_tool;
#[cfg(feature = "code-mode")]
pub mod code_mode;
mod error;
pub mod firecrawl;
mod http;
pub mod jev;
mod observability;
mod sse;
mod structured;
#[cfg(test)]
mod test_support;
mod tool_loop;
mod types;

pub use accumulating_stream::AccumulatingStream;
pub use anthropic::Anthropic;
pub use client_tool::ClientTool;
pub use error::{Error, ErrorCode, ErrorKind, Result, Service};
pub use firecrawl::{FirecrawlClient, FirecrawlFetchPage, FirecrawlWebSearch};
pub use sse::{SseEvent, parse_message_stream_event, process_message_stream_sse, process_sse};
pub use structured::Parsed;
pub use tool_loop::{ToolRun, ToolRunError};
pub use types::*;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}
