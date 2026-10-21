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
mod backoff;
mod batch_wait;
mod client;
#[cfg(feature = "code-mode")]
pub mod code_mode;
mod error;
pub mod firecrawl;
pub mod jev;
mod observability;
mod sse;
mod structured;
#[cfg(test)]
mod test_support;
mod tool;
mod tool_loop;
mod types;

pub use accumulating_stream::AccumulatingStream;
pub use client::Anthropic;
pub use error::{Error, HttpMetadata, Result};
pub use firecrawl::{FirecrawlClient, FirecrawlFetchPage, FirecrawlWebSearch};
pub use sse::{SseEvent, parse_message_stream_event, process_message_stream_sse, process_sse};
pub use structured::Parsed;
pub use tool::ClientTool;
pub use tool_loop::ToolRun;
pub use types::*;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}

/// Pushes a message to the messages vector, or merges it with the last message if they have the same role.
///
/// This function helps maintain a clean message history by combining consecutive messages
/// from the same role into a single message entry.
pub fn push_or_merge_message(messages: &mut Vec<MessageParam>, to_push: MessageParam) {
    if let Some(last) = messages.last_mut() {
        if last.role != to_push.role {
            messages.push(to_push);
        } else {
            merge_message_content(&mut last.content, to_push.content);
        }
    } else {
        messages.push(to_push);
    }
}

/// Merges new message content into existing message content.
///
/// Handles all combinations of string and array content types, converting between
/// them as necessary to produce a unified message content.
pub fn merge_message_content(existing: &mut MessageParamContent, new: MessageParamContent) {
    match (&mut *existing, new) {
        (MessageParamContent::Array(existing_blocks), MessageParamContent::Array(new_blocks)) => {
            existing_blocks.extend(new_blocks);
        }
        (MessageParamContent::Array(existing_blocks), MessageParamContent::String(new_string)) => {
            existing_blocks.push(ContentBlock::Text(crate::TextBlock::new(new_string)));
        }
        (MessageParamContent::String(existing_string), MessageParamContent::Array(new_blocks)) => {
            let mut combined = vec![ContentBlock::Text(crate::TextBlock::new(
                existing_string.clone(),
            ))];
            combined.extend(new_blocks);
            *existing = MessageParamContent::Array(combined);
        }
        (MessageParamContent::String(existing_string), MessageParamContent::String(new_string)) => {
            existing_string.push_str(&new_string);
        }
    }
}
