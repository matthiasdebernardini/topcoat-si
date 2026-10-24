//! Waiting for a message batch to end and collecting its results, and the one-request batch
//! helper built on them.

use std::collections::HashMap;
use std::time::Duration;

use time::OffsetDateTime;

use crate::{
    Anthropic, Error, ErrorKind, Message, MessageBatch, MessageBatchCreateParams,
    MessageBatchCreateRequest, MessageBatchProcessingStatus, MessageBatchResultVariant,
    MessageCreateParams, Result,
};

/// How many polls in a row may fail with a retryable error before the wait gives up.
const MAX_CONSECUTIVE_POLL_FAILURES: usize = 5;

/// What [`BatchWait::on_poll`] tells [`Anthropic::await_one`] to do next.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Control {
    /// Keep waiting.
    Continue,
    /// Cancel the batch, then wait for it to end.
    Cancel,
}

/// How [`Anthropic::await_one`] waits.
pub struct BatchWait<'a> {
    /// The time between polls. Must be greater than zero.
    pub poll: Duration,
    /// When to stop waiting, as wall-clock time so it survives a restart. `None` waits until
    /// the batch ends, which the API bounds at 24 hours.
    pub deadline: Option<OffsetDateTime>,
    /// Called after each poll that finds the batch still running, including after a cancel, so
    /// a progress display keeps updating. Returning [`Control::Cancel`] sends `/cancel`; once a
    /// cancel has been accepted, later `Cancel`s are ignored. A cancel that fails is logged and
    /// asked for again on the next poll.
    /// It must be `Send` so the wait can run on a spawned task.
    pub on_poll: &'a mut (dyn FnMut(&MessageBatch) -> Control + Send),
}

/// How a one-request batch ended.
#[derive(Debug, Clone)]
// `Message` carries the response fields the API added; there is one outcome per wait.
#[allow(clippy::large_enum_variant)]
#[non_exhaustive]
pub enum BatchOutcome {
    /// The request produced a message.
    Succeeded(Message),
    /// The request failed. The error keeps the API's error type, so
    /// [`Error::is_retryable`] says whether submitting it again might work.
    Errored(Error),
    /// The batch was canceled before the request ran.
    Canceled,
    /// The batch expired before the request ran.
    Expired,
    /// The deadline passed while the batch was still running. The batch was not canceled.
    DeadlinePassed,
}

/// The end of [`Anthropic::await_one`]: the outcome and the batch as last polled.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BatchDone {
    /// The ended batch, or for [`BatchOutcome::DeadlinePassed`] the last poll. Its `ended_at`
    /// minus `created_at` is the time the batch took.
    pub batch: MessageBatch,
    /// How the request ended.
    pub outcome: BatchOutcome,
}

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
        check_poll_interval(poll_interval)?;
        let mut failures = 0;
        loop {
            if let Some(batch) = self
                .poll_message_batch(message_batch_id, &mut failures)
                .await?
                && batch.processing_status == MessageBatchProcessingStatus::Ended
            {
                return Ok(batch);
            }
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Submits `params` as a batch of one request under `custom_id`.
    ///
    /// Pair it with [`Anthropic::await_one`]. They are separate calls so the batch id can be
    /// saved in between, and the wait resumed after a restart.
    ///
    /// # Errors
    ///
    /// Any error from [`Anthropic::create_message_batch`], including a validation error for an
    /// invalid `custom_id` or for `params` that stream.
    pub async fn submit_one(
        &self,
        custom_id: impl Into<String>,
        params: MessageCreateParams,
    ) -> Result<MessageBatch> {
        let request = MessageBatchCreateRequest::new(custom_id, params);
        self.create_message_batch(MessageBatchCreateParams::new(vec![request]))
            .await
    }

    /// Waits for the one-request batch `message_batch_id` to end and returns the result for
    /// `custom_id`.
    ///
    /// Polls every `wait.poll`, retrying failed polls like [`Anthropic::wait_for_message_batch`].
    /// After each poll that finds the batch still running it calls `wait.on_poll`; on
    /// [`Control::Cancel`] it sends `/cancel` and keeps waiting for the batch to end. A failed
    /// cancel is logged and does not end the wait.
    ///
    /// Once `wait.deadline` has passed and the batch is still running, it returns
    /// [`BatchOutcome::DeadlinePassed`] without calling `wait.on_poll` and without cancelling.
    /// After a cancel has been accepted, or once a poll finds the batch canceling, the deadline
    /// no longer applies and no cancel is sent: a canceling batch ends within seconds, and
    /// `DeadlinePassed` always means the batch was not canceled.
    ///
    /// # Errors
    ///
    /// A validation error, before any request, when `wait.poll` is zero; the error from a poll
    /// or the results that ended the wait; and a decode error when the results hold
    /// no result for `custom_id`, or one of a type this crate does not know.
    pub async fn await_one(
        &self,
        message_batch_id: &str,
        custom_id: &str,
        wait: BatchWait<'_>,
    ) -> Result<BatchDone> {
        check_poll_interval(wait.poll)?;
        let mut failures = 0;
        let mut canceled = false;
        let mut last = None;
        loop {
            let deadline_passed = !canceled
                && wait
                    .deadline
                    .is_some_and(|deadline| OffsetDateTime::now_utc() >= deadline);
            match self
                .poll_message_batch(message_batch_id, &mut failures)
                .await?
            {
                Some(batch) if batch.processing_status == MessageBatchProcessingStatus::Ended => {
                    return self.one_result(batch, custom_id).await;
                }
                Some(batch) => {
                    // A canceling batch was canceled already: by a cancel that timed out here but
                    // reached the server, or before a restart.
                    if batch.processing_status == MessageBatchProcessingStatus::Canceling {
                        canceled = true;
                    }
                    if deadline_passed && !canceled {
                        return Ok(BatchDone {
                            batch,
                            outcome: BatchOutcome::DeadlinePassed,
                        });
                    }
                    let control = (wait.on_poll)(&batch);
                    if !canceled && control == Control::Cancel {
                        match self.cancel_message_batch(message_batch_id).await {
                            Ok(batch)
                                if batch.processing_status
                                    == MessageBatchProcessingStatus::Ended =>
                            {
                                return self.one_result(batch, custom_id).await;
                            }
                            Ok(_) => canceled = true,
                            Err(error) => tracing::warn!(
                                target: "carpa::client",
                                error = &error as &dyn std::error::Error,
                                batch_id = message_batch_id,
                                "cancelling the message batch failed; polling again"
                            ),
                        }
                    }
                    last = Some(batch);
                }
                None => {
                    if deadline_passed && let Some(batch) = last.take() {
                        return Ok(BatchDone {
                            batch,
                            outcome: BatchOutcome::DeadlinePassed,
                        });
                    }
                }
            }
            tokio::time::sleep(wait.poll).await;
        }
    }

    /// Reads the result for `custom_id` from `batch`, which has ended.
    async fn one_result(&self, batch: MessageBatch, custom_id: &str) -> Result<BatchDone> {
        let message_batch_id = batch.id.as_str();
        let mut results = self.message_batch_results_by_id(message_batch_id).await?;
        let outcome = match results.remove(custom_id) {
            Some(MessageBatchResultVariant::Succeeded { message }) => {
                Ok(BatchOutcome::Succeeded(message))
            }
            Some(MessageBatchResultVariant::Errored { error }) => {
                Ok(BatchOutcome::Errored(error.into()))
            }
            Some(MessageBatchResultVariant::Canceled) => Ok(BatchOutcome::Canceled),
            Some(MessageBatchResultVariant::Expired) => Ok(BatchOutcome::Expired),
            Some(_) => Err(Error::new(
                ErrorKind::Decode,
                format!("batch {message_batch_id} has a result of an unknown type for {custom_id}"),
            )),
            None => Err(Error::new(
                ErrorKind::Decode,
                format!("batch {message_batch_id} has no result for {custom_id}"),
            )),
        }?;
        Ok(BatchDone { batch, outcome })
    }

    /// Polls the batch once. A retryable failure gives `None` and counts toward the limit of
    /// consecutive failures; reaching the limit, or any other failure, is the error. A
    /// successful poll resets the count.
    async fn poll_message_batch(
        &self,
        message_batch_id: &str,
        failures: &mut usize,
    ) -> Result<Option<MessageBatch>> {
        match self.get_message_batch(message_batch_id).await {
            Ok(batch) => {
                *failures = 0;
                if batch.processing_status != MessageBatchProcessingStatus::Ended {
                    tracing::debug!(
                        batch_id = %batch.id,
                        processing = batch.request_counts.processing,
                        "message batch still processing"
                    );
                }
                Ok(Some(batch))
            }
            Err(error) if error.is_retryable() => {
                *failures += 1;
                if *failures >= MAX_CONSECUTIVE_POLL_FAILURES {
                    return Err(error);
                }
                tracing::warn!(
                    target: "carpa::client",
                    error = &error as &dyn std::error::Error,
                    failures = *failures,
                    "polling the message batch failed; polling again"
                );
                Ok(None)
            }
            Err(error) => Err(error),
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

/// Rejects a zero poll interval, which would poll without pause.
fn check_poll_interval(poll_interval: Duration) -> Result<()> {
    if poll_interval.is_zero() {
        return Err(Error::validation(
            "poll_interval must be greater than zero",
            Some("poll_interval".to_string()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
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

    const MESSAGE: &str = r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#;

    fn results(custom_id: &str, result: &str) -> Canned {
        let body = format!("{{\"custom_id\":\"{custom_id}\",\"result\":{result}}}\n");
        Canned::json(200, body).with_content_type("application/x-jsonl")
    }

    fn succeeded() -> String {
        format!(r#"{{"type":"succeeded","message":{MESSAGE}}}"#)
    }

    fn params() -> MessageCreateParams {
        MessageCreateParams::simple("2+2?", crate::KnownModel::ClaudeHaiku55)
    }

    async fn await_one(
        client: &Anthropic,
        deadline: Option<OffsetDateTime>,
        on_poll: &mut (dyn FnMut(&MessageBatch) -> Control + Send),
    ) -> Result<BatchOutcome> {
        await_done(client, Duration::from_millis(1), deadline, on_poll)
            .await
            .map(|done| done.outcome)
    }

    async fn await_done(
        client: &Anthropic,
        poll: Duration,
        deadline: Option<OffsetDateTime>,
        on_poll: &mut (dyn FnMut(&MessageBatch) -> Control + Send),
    ) -> Result<BatchDone> {
        let wait = BatchWait {
            poll,
            deadline,
            on_poll,
        };
        client.await_one("msgbatch_1", "turn-1", wait).await
    }

    #[tokio::test]
    async fn await_one_runs_on_a_spawned_task() {
        // An app awaits this from a tokio::spawn'ed job, so the future must be Send.
        let task = tokio::spawn(async {
            let client = client("http://127.0.0.1:9".into()).with_max_retries(0);
            let mut on_poll = |_: &MessageBatch| Control::Continue;
            await_one(&client, None, &mut on_poll).await.is_err()
        });
        assert!(task.await.unwrap());
    }

    fn request_lines(recorded: &Mutex<Vec<crate::test_support::Recorded>>) -> Vec<String> {
        recorded
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.headers.lines().next().unwrap().to_string())
            .collect()
    }

    const POLL_LINE: &str = "GET /v1/messages/batches/msgbatch_1 HTTP/1.1";
    const CANCEL_LINE: &str = "POST /v1/messages/batches/msgbatch_1/cancel HTTP/1.1";
    const RESULTS_LINE: &str = "GET /v1/messages/batches/msgbatch_1/results HTTP/1.1";

    #[tokio::test]
    async fn submit_then_await_returns_the_message() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("ended")),
            results("turn-1", &succeeded()),
        ])
        .await;
        let client = client(base_url);

        let submitted = client.submit_one("turn-1", params()).await.unwrap();
        let mut polls = 0;
        let done = await_done(&client, Duration::from_millis(1), None, &mut |batch| {
            assert_eq!(
                batch.processing_status,
                MessageBatchProcessingStatus::InProgress
            );
            polls += 1;
            Control::Continue
        })
        .await
        .unwrap();

        assert_eq!(submitted.id, "msgbatch_1");
        assert!(matches!(done.outcome, BatchOutcome::Succeeded(message) if message.id == "msg_1"));
        assert_eq!(
            done.batch.processing_status,
            MessageBatchProcessingStatus::Ended
        );
        assert_eq!(polls, 1);
        let requests = recorded.lock().unwrap();
        let lines: Vec<_> = requests
            .iter()
            .map(|request| request.headers.lines().next().unwrap().to_string())
            .collect();
        assert_eq!(
            lines,
            [
                "POST /v1/messages/batches HTTP/1.1",
                "GET /v1/messages/batches/msgbatch_1 HTTP/1.1",
                "GET /v1/messages/batches/msgbatch_1 HTTP/1.1",
                "GET /v1/messages/batches/msgbatch_1/results HTTP/1.1",
            ]
        );
        let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
        assert_eq!(body["requests"].as_array().unwrap().len(), 1);
        assert_eq!(body["requests"][0]["custom_id"], "turn-1");
        assert!(body["requests"][0]["params"].get("stream").is_none());
    }

    #[tokio::test]
    async fn submit_rejects_streaming_params_without_a_request() {
        let (base_url, recorded) = serve(Vec::new()).await;

        let error = client(base_url)
            .submit_one("turn-1", params().with_stream(true))
            .await
            .unwrap_err();

        assert!(matches!(error.kind(), ErrorKind::Validation { .. }));
        assert!(recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_overloaded_result_is_a_retryable_error() {
        let errored = r#"{"type":"errored","error":{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}}"#;
        let (base_url, _) = serve(vec![
            Canned::json(200, batch("ended")),
            results("turn-1", errored),
        ])
        .await;

        let outcome = await_one(&client(base_url), None, &mut |_| Control::Continue)
            .await
            .unwrap();

        let BatchOutcome::Errored(error) = outcome else {
            panic!("expected an errored outcome, got {outcome:?}");
        };
        assert!(error.is_retryable());
        assert!(matches!(
            error.kind(),
            ErrorKind::Api {
                error_type: Some(crate::ErrorType::OverloadedError),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn an_expired_result_is_expired() {
        let (base_url, _) = serve(vec![
            Canned::json(200, batch("ended")),
            results("turn-1", r#"{"type":"expired"}"#),
        ])
        .await;

        let outcome = await_one(&client(base_url), None, &mut |_| Control::Continue)
            .await
            .unwrap();

        assert!(matches!(outcome, BatchOutcome::Expired));
    }

    #[tokio::test]
    async fn cancel_from_on_poll_sends_cancel_once_and_keeps_reporting_polls() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("canceling")),
            Canned::json(200, batch("canceling")),
            Canned::json(200, batch("canceling")),
            Canned::json(200, batch("ended")),
            results("turn-1", r#"{"type":"canceled"}"#),
        ])
        .await;

        let mut polls = 0;
        let outcome = await_one(&client(base_url), None, &mut |_| {
            polls += 1;
            Control::Cancel
        })
        .await
        .unwrap();

        assert!(matches!(outcome, BatchOutcome::Canceled));
        assert_eq!(polls, 3);
        assert_eq!(
            request_lines(&recorded),
            [
                POLL_LINE,
                CANCEL_LINE,
                POLL_LINE,
                POLL_LINE,
                POLL_LINE,
                RESULTS_LINE
            ]
        );
    }

    #[tokio::test]
    async fn a_cancel_that_answers_ended_reads_the_result_at_once() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("ended")),
            results("turn-1", r#"{"type":"canceled"}"#),
        ])
        .await;

        let done = await_done(
            &client(base_url),
            Duration::from_millis(1),
            None,
            &mut |_| Control::Cancel,
        )
        .await
        .unwrap();

        assert!(matches!(done.outcome, BatchOutcome::Canceled));
        assert_eq!(
            done.batch.processing_status,
            MessageBatchProcessingStatus::Ended
        );
        assert_eq!(
            request_lines(&recorded),
            [POLL_LINE, CANCEL_LINE, RESULTS_LINE]
        );
    }

    #[tokio::test]
    async fn a_failed_cancel_keeps_waiting_and_asks_again() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("in_progress")),
            unavailable(),
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("canceling")),
            Canned::json(200, batch("ended")),
            results("turn-1", r#"{"type":"canceled"}"#),
        ])
        .await;

        let mut polls = 0;
        let outcome = await_one(&client(base_url).with_max_retries(0), None, &mut |_| {
            polls += 1;
            Control::Cancel
        })
        .await
        .unwrap();

        assert!(matches!(outcome, BatchOutcome::Canceled));
        assert_eq!(polls, 2);
        assert_eq!(
            request_lines(&recorded),
            [
                POLL_LINE,
                CANCEL_LINE,
                POLL_LINE,
                CANCEL_LINE,
                POLL_LINE,
                RESULTS_LINE
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_cancel_then_an_ended_poll_returns_the_result() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("in_progress")),
            unavailable(),
            Canned::json(200, batch("ended")),
            results("turn-1", &succeeded()),
        ])
        .await;

        let outcome = await_one(&client(base_url).with_max_retries(0), None, &mut |_| {
            Control::Cancel
        })
        .await
        .unwrap();

        assert!(matches!(outcome, BatchOutcome::Succeeded(_)));
        assert_eq!(
            request_lines(&recorded),
            [POLL_LINE, CANCEL_LINE, POLL_LINE, RESULTS_LINE]
        );
    }

    #[tokio::test]
    async fn a_passed_deadline_returns_without_cancelling() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("ended")),
        ])
        .await;

        let deadline = OffsetDateTime::now_utc() - time::Duration::seconds(1);
        let mut polls = 0;
        let done = await_done(
            &client(base_url),
            Duration::from_millis(1),
            Some(deadline),
            &mut |_| {
                polls += 1;
                Control::Cancel
            },
        )
        .await
        .unwrap();

        assert!(matches!(done.outcome, BatchOutcome::DeadlinePassed));
        assert_eq!(
            done.batch.processing_status,
            MessageBatchProcessingStatus::InProgress
        );
        assert_eq!(polls, 0);
        assert_eq!(request_lines(&recorded), [POLL_LINE]);
    }

    #[tokio::test]
    async fn a_batch_found_canceling_ignores_the_deadline_and_sends_no_cancel() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("canceling")),
            Canned::json(200, batch("ended")),
            results("turn-1", r#"{"type":"canceled"}"#),
        ])
        .await;

        let deadline = OffsetDateTime::now_utc() - time::Duration::seconds(1);
        let outcome = await_done(
            &client(base_url),
            Duration::from_millis(1),
            Some(deadline),
            &mut |_| Control::Cancel,
        )
        .await
        .unwrap()
        .outcome;

        assert!(matches!(outcome, BatchOutcome::Canceled));
        assert_eq!(
            request_lines(&recorded),
            [POLL_LINE, POLL_LINE, RESULTS_LINE]
        );
    }

    #[tokio::test]
    async fn a_passed_deadline_after_a_failed_poll_returns_the_next_batch() {
        let (base_url, recorded) = serve(vec![
            unavailable(),
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("ended")),
        ])
        .await;

        let deadline = OffsetDateTime::now_utc() - time::Duration::seconds(1);
        let mut polls = 0;
        let done = await_done(
            &client(base_url).with_max_retries(0),
            Duration::from_millis(1),
            Some(deadline),
            &mut |_| {
                polls += 1;
                Control::Cancel
            },
        )
        .await
        .unwrap();

        assert!(matches!(done.outcome, BatchOutcome::DeadlinePassed));
        assert_eq!(
            done.batch.processing_status,
            MessageBatchProcessingStatus::InProgress
        );
        assert_eq!(polls, 0);
        assert_eq!(request_lines(&recorded), [POLL_LINE, POLL_LINE]);
    }

    #[tokio::test]
    async fn the_deadline_is_ignored_once_a_cancel_is_accepted() {
        let (base_url, recorded) = serve(vec![
            Canned::json(200, batch("in_progress")),
            Canned::json(200, batch("canceling")),
            Canned::json(200, batch("canceling")),
            Canned::json(200, batch("ended")),
            results("turn-1", r#"{"type":"canceled"}"#),
        ])
        .await;

        // The first poll comes well before the deadline; the second, after the cancel, well
        // after it.
        let deadline = OffsetDateTime::now_utc() + time::Duration::milliseconds(200);
        let outcome = await_done(
            &client(base_url),
            Duration::from_millis(400),
            Some(deadline),
            &mut |_| Control::Cancel,
        )
        .await
        .unwrap()
        .outcome;

        assert!(matches!(outcome, BatchOutcome::Canceled));
        assert_eq!(
            request_lines(&recorded),
            [POLL_LINE, CANCEL_LINE, POLL_LINE, POLL_LINE, RESULTS_LINE]
        );
    }

    #[tokio::test]
    async fn a_missing_result_is_a_decode_error() {
        let (base_url, _) = serve(vec![
            Canned::json(200, batch("ended")),
            results("other", &succeeded()),
        ])
        .await;

        let error = await_one(&client(base_url), None, &mut |_| Control::Continue)
            .await
            .unwrap_err();

        assert_eq!(error.kind(), &ErrorKind::Decode);
    }

    #[tokio::test]
    async fn await_rejects_zero_interval_without_a_request() {
        let (base_url, recorded) = serve(Vec::new()).await;

        let wait = BatchWait {
            poll: Duration::ZERO,
            deadline: None,
            on_poll: &mut |_| Control::Continue,
        };
        let error = client(base_url)
            .await_one("msgbatch_1", "turn-1", wait)
            .await
            .unwrap_err();

        assert!(matches!(error.kind(), ErrorKind::Validation { .. }));
        assert!(recorded.lock().unwrap().is_empty());
    }
}
