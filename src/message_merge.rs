//! Merging consecutive same-role messages into one.

use crate::{ContentBlock, MessageParam, MessageParamContent, TextBlock};

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
            existing_blocks.push(ContentBlock::Text(TextBlock::new(new_string)));
        }
        (MessageParamContent::String(existing_string), MessageParamContent::Array(new_blocks)) => {
            let mut combined = vec![ContentBlock::Text(TextBlock::new(existing_string.clone()))];
            combined.extend(new_blocks);
            *existing = MessageParamContent::Array(combined);
        }
        (MessageParamContent::String(existing_string), MessageParamContent::String(new_string)) => {
            existing_string.push_str(&new_string);
        }
    }
}
