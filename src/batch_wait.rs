//! Waiting for a message batch to end and collecting its results.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;

use crate::{
    Anthropic, Error, MessageBatch, MessageBatchProcessingStatus, MessageBatchResultVariant, Result,
};

impl Anthropic {
    /// Polls [`Anthropic::get_message_batch`] every `poll_interval` until the batch has ended, and
    /// returns its final state.
    ///
    /// There is no overall timeout, and a batch can take up to 24 hours; wrap the call in
    /// `tokio::time::timeout` to bound it. Each poll retries like any other request, and the first
    /// poll error ends the wait.
    ///
    /// # Errors
    ///
    /// A validation error, before any request, when `poll_interval` is zero; otherwise any error
    /// from [`Anthropic::get_message_batch`].
    pub async fn wait_for_message_batch(
        &self,
        message_batch_id: &str,
        poll_interval: Duration,
    ) -> Result<MessageBatch> {
        if poll_interval.is_zero() {
            return Err(Error::validation(
                "poll_interval must be greater than zero",
                Some("poll_interval".to_string()),
            ));
        }
        loop {
            let batch = self.get_message_batch(message_batch_id).await?;
            if batch.processing_status == MessageBatchProcessingStatus::Ended {
                return Ok(batch);
            }
            tracing::debug!(
                batch_id = %batch.id,
                processing = batch.request_counts.processing,
                "message batch still processing"
            );
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Collects an ended batch's results keyed by `custom_id`.
    ///
    /// Reads the whole results file into memory; for very large batches, consume
    /// [`Anthropic::stream_message_batch_results`] instead. Results come back in no particular
    /// order, which is why they are keyed. If a `custom_id` repeats, the last result wins.
    ///
    /// # Errors
    ///
    /// Any error from [`Anthropic::stream_message_batch_results`], including one for a malformed
    /// line part way through.
    pub async fn message_batch_results_by_id(
        &self,
        message_batch_id: &str,
    ) -> Result<HashMap<String, MessageBatchResultVariant>> {
        let mut results =
            std::pin::pin!(self.stream_message_batch_results(message_batch_id).await?);
        let mut by_id = HashMap::new();
        while let Some(result) = results.next().await {
            let result = result?;
            by_id.insert(result.custom_id, result.result);
        }
        Ok(by_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Canned, serve};

    fn client(base_url: String) -> Anthropic {
        Anthropic::new(Some("test_key".to_string()))
            .unwrap()
            .with_base_url(base_url)
            .unwrap()
    }

    fn batch(status: &str) -> String {
        format!(
            r#"{{"id":"msgbatch_1","type":"message_batch","processing_status":"{status}",
            "request_counts":{{"processing":1,"succeeded":0,"errored":0,"canceled":0,"expired":0}},
            "ended_at":null,"created_at":"2024-09-24T18:37:24Z","expires_at":"2024-09-25T18:37:24Z",
            "cancel_initiated_at":null,"results_url":null}}"#
        )
    }

    #[tokio::test]
    async fn wait_polls_until_ended() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("canceling")),
            Canned::json(200, batch("ended")),
        ])
        .await;

        let ended = client(base_url)
            .wait_for_message_batch("msgbatch_1", Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(ended.processing_status, MessageBatchProcessingStatus::Ended);
        let requests = recorded.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in requests.iter() {
            assert!(
                request
                    .headers
                    .starts_with("GET /v1/messages/batches/msgbatch_1 "),
                "{}",
                request.headers
            );
        }
    }

    #[tokio::test]
    async fn wait_rejects_zero_interval_without_a_request() {
        let (base_url, recorded) = serve(Vec::new()).await;

        let error = client(base_url)
            .wait_for_message_batch("msgbatch_1", Duration::ZERO)
            .await
            .unwrap_err();

        assert!(error.is_validation());
        assert!(recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn results_are_keyed_by_custom_id() {
        let message = r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#;
        let body = format!(
            "{{\"custom_id\":\"second\",\"result\":{{\"type\":\"expired\"}}}}\n\
             {{\"custom_id\":\"first\",\"result\":{{\"type\":\"succeeded\",\"message\":{message}}}}}\n"
        );
        let (base_url, recorded) = serve(vec![
            Canned::json(200, body).with_content_type("application/x-jsonl"),
        ])
        .await;

        let results = client(base_url)
            .message_batch_results_by_id("msgbatch_1")
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(matches!(
            results.get("first"),
            Some(MessageBatchResultVariant::Succeeded { message }) if message.id == "msg_1"
        ));
        assert_eq!(
            results.get("second"),
            Some(&MessageBatchResultVariant::Expired)
        );
        assert!(
            recorded.lock().unwrap()[0]
                .headers
                .starts_with("GET /v1/messages/batches/msgbatch_1/results ")
        );
    }
}
