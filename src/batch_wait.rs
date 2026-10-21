//! Waiting for a message batch to end and collecting its results.

use std::collections::HashMap;
use std::time::Duration;

use crate::{
    Anthropic, Error, MessageBatch, MessageBatchProcessingStatus, MessageBatchResultVariant, Result,
};

/// How many polls in a row may fail with a retryable error before the wait gives up.
const MAX_CONSECUTIVE_POLL_FAILURES: usize = 5;

impl Anthropic {
    /// Polls [`Anthropic::get_message_batch`] every `poll_interval` until the batch has ended, and
    /// returns its final state.
    ///
    /// There is no overall timeout, and a batch can take up to 24 hours; wrap the call in
    /// `tokio::time::timeout` to bound it. Each poll retries like any other request. A poll that
    /// still fails with a retryable error (see [`Error::is_retryable`]) is tried again after
    /// `poll_interval`; the wait gives up with that error after 5 such failures in a row, and any
    /// successful poll resets the count. An error that is not retryable ends the wait at once.
    ///
    /// # Errors
    ///
    /// A validation error, before any request, when `poll_interval` is zero; otherwise the error
    /// from [`Anthropic::get_message_batch`] that ended the wait.
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
        let mut failures = 0;
        loop {
            match self.get_message_batch(message_batch_id).await {
                Ok(batch) => {
                    failures = 0;
                    if batch.processing_status == MessageBatchProcessingStatus::Ended {
                        return Ok(batch);
                    }
                    tracing::debug!(
                        batch_id = %batch.id,
                        processing = batch.request_counts.processing,
                        "message batch still processing"
                    );
                }
                Err(error) if error.is_retryable() => {
                    failures += 1;
                    if failures >= MAX_CONSECUTIVE_POLL_FAILURES {
                        return Err(error);
                    }
                    tracing::warn!(
                        target: "topcoat_si::client",
                        error = &error as &dyn std::error::Error,
                        failures,
                        "polling the message batch failed; polling again"
                    );
                }
                Err(error) => return Err(error),
            }
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
        use futures::StreamExt;

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
    use crate::ErrorKind;
    use crate::test_support::{Canned, serve};

    fn client(base_url: String) -> Anthropic {
        Anthropic::try_new(Some("test_key".to_string()))
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

        assert!(matches!(error.kind(), ErrorKind::Validation { .. }));
        assert!(recorded.lock().unwrap().is_empty());
    }

    fn unavailable() -> Canned {
        Canned::json(503, r#"{"error":{"type":"api_error","message":"down"}}"#)
    }

    #[tokio::test]
    async fn wait_keeps_polling_through_retryable_errors() {
        let (base_url, recorded) = serve(vec![
            unavailable(),
            unavailable(),
            Canned::json(200, batch("ended")),
        ])
        .await;

        let ended = client(base_url)
            .with_max_retries(0)
            .wait_for_message_batch("msgbatch_1", Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(ended.processing_status, MessageBatchProcessingStatus::Ended);
        assert_eq!(recorded.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn wait_gives_up_after_five_consecutive_failures() {
        let (base_url, recorded) = serve(vec![
            unavailable(),
            unavailable(),
            unavailable(),
            unavailable(),
            unavailable(),
            Canned::json(200, batch("ended")),
        ])
        .await;

        let error = client(base_url)
            .with_max_retries(0)
            .wait_for_message_batch("msgbatch_1", Duration::from_millis(1))
            .await
            .unwrap_err();

        assert_eq!(error.status(), Some(503));
        assert_eq!(recorded.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn a_successful_poll_resets_the_failure_count() {
        let mut responses = vec![unavailable(); 4];
        responses.push(Canned::json(200, batch("in_progress")));
        responses.extend(vec![unavailable(); 4]);
        responses.push(Canned::json(200, batch("ended")));
        let (base_url, recorded) = serve(responses).await;

        let ended = client(base_url)
            .with_max_retries(0)
            .wait_for_message_batch("msgbatch_1", Duration::from_millis(1))
            .await
            .unwrap();

        assert_eq!(ended.processing_status, MessageBatchProcessingStatus::Ended);
        assert_eq!(recorded.lock().unwrap().len(), 10);
    }

    #[tokio::test]
    async fn wait_stops_at_once_on_a_non_retryable_error() {
        let (base_url, recorded) = serve(vec![
            Canned::json(
                401,
                r#"{"error":{"type":"authentication_error","message":"no"}}"#,
            ),
            Canned::json(200, batch("ended")),
        ])
        .await;

        let error = client(base_url)
            .with_max_retries(0)
            .wait_for_message_batch("msgbatch_1", Duration::from_millis(1))
            .await
            .unwrap_err();

        assert_eq!(error.status(), Some(401));
        assert_eq!(recorded.lock().unwrap().len(), 1);
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
