//! Tool runs and Message Batch ids that survive a restart.

use futures::{Stream, StreamExt, future};
use surrealdb::types::{Action, Datetime, RecordId, SurrealValue};
use surrealdb::{Connection, Surreal};
use time::OffsetDateTime;

use crate::error::{Error, Result};
use crate::surreal::store_error;
use crate::surreal::time::{from_datetime, to_datetime};
use crate::tool_loop::{ToolRun, ToolRunError};
use crate::types::{Message, MessageBatch, MessageBatchProcessingStatus, MessageParam};

const TRANSCRIPT_TABLE: &str = "topcoat_transcript";
const BATCH_TABLE: &str = "topcoat_batch";

/// A saved tool run: the conversation and every response, plus the error if the run failed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Transcript {
    /// The id the run was saved under.
    pub id: String,
    /// The whole conversation, ready to extend and send again.
    pub messages: Vec<MessageParam>,
    /// Every response received, in order.
    pub responses: Vec<Message>,
    /// What ended the run, when it failed.
    pub error: Option<String>,
    /// When the record was last written.
    pub updated_at: OffsetDateTime,
}

#[derive(SurrealValue)]
struct TranscriptRow {
    messages: String,
    responses: String,
    error: Option<String>,
    updated_at: Datetime,
}

impl TranscriptRow {
    fn into_transcript(self, id: &str) -> Result<Transcript> {
        let decode = |what: &str, source: serde_json::Error| {
            Error::store(
                format!("decoding {what} of transcript {id}"),
                Some(Box::new(source)),
            )
        };
        Ok(Transcript {
            id: id.to_owned(),
            messages: serde_json::from_str(&self.messages)
                .map_err(|source| decode("messages", source))?,
            responses: serde_json::from_str(&self.responses)
                .map_err(|source| decode("responses", source))?,
            error: self.error,
            updated_at: from_datetime(&self.updated_at),
        })
    }
}

/// Saves tool runs under an id of your choosing, so a run that was cut short can be picked up
/// after a restart.
///
/// Messages and responses are stored as JSON text: nothing is lost, and the record stays small
/// to index.
#[derive(Debug, Clone)]
pub struct Transcripts<C: Connection> {
    db: Surreal<C>,
}

impl<C: Connection> Transcripts<C> {
    /// Defines the transcript table if it is missing and keeps `db`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the schema cannot be defined.
    pub async fn try_new(db: Surreal<C>) -> Result<Self> {
        db.query(
            "DEFINE TABLE IF NOT EXISTS topcoat_transcript TYPE NORMAL SCHEMAFULL;
             DEFINE FIELD IF NOT EXISTS messages ON topcoat_transcript TYPE string;
             DEFINE FIELD IF NOT EXISTS responses ON topcoat_transcript TYPE string;
             DEFINE FIELD IF NOT EXISTS error ON topcoat_transcript TYPE option<string>;
             DEFINE FIELD IF NOT EXISTS response_count ON topcoat_transcript TYPE int;
             DEFINE FIELD IF NOT EXISTS updated_at ON topcoat_transcript TYPE datetime;",
        )
        .await
        .and_then(|response| response.check())
        .map_err(|error| store_error("defining the transcript table", error))?;
        Ok(Self { db })
    }

    /// Saves a finished run under `id`, replacing any earlier save.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the run cannot be encoded or written.
    pub async fn save(&self, id: &str, run: &ToolRun) -> Result<()> {
        self.write(id, &run.messages, &run.responses, None).await
    }

    /// Saves a failed run under `id`, with the error text, so it can be resumed from
    /// `messages`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the run cannot be encoded or written.
    pub async fn save_failed(&self, id: &str, failed: &ToolRunError) -> Result<()> {
        self.write(
            id,
            &failed.messages,
            &failed.responses,
            Some(failed.error.to_string()),
        )
        .await
    }

    async fn write(
        &self,
        id: &str,
        messages: &[MessageParam],
        responses: &[Message],
        error: Option<String>,
    ) -> Result<()> {
        let encode = |what: &str, source: serde_json::Error| {
            Error::store(
                format!("encoding {what} of transcript {id}"),
                Some(Box::new(source)),
            )
        };
        let messages_json =
            serde_json::to_string(messages).map_err(|source| encode("messages", source))?;
        let responses_json =
            serde_json::to_string(responses).map_err(|source| encode("responses", source))?;
        self.db
            .query(
                "UPSERT $id SET messages = $messages, responses = $responses, error = $error, \
                 response_count = $count, updated_at = time::now()",
            )
            .bind(("id", RecordId::new(TRANSCRIPT_TABLE, id)))
            .bind(("messages", messages_json))
            .bind(("responses", responses_json))
            .bind(("error", error))
            .bind(("count", responses.len() as i64))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error(format!("saving transcript {id}"), error))?;
        Ok(())
    }

    /// The transcript saved under `id`, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the read fails or the saved JSON does
    /// not parse.
    pub async fn load(&self, id: &str) -> Result<Option<Transcript>> {
        let mut response = self
            .db
            .query("SELECT messages, responses, error, updated_at FROM $id")
            .bind(("id", RecordId::new(TRANSCRIPT_TABLE, id)))
            .await
            .map_err(|error| store_error(format!("loading transcript {id}"), error))?;
        let rows: Vec<TranscriptRow> = response
            .take(0)
            .map_err(|error| store_error(format!("loading transcript {id}"), error))?;
        rows.into_iter()
            .next()
            .map(|row| row.into_transcript(id))
            .transpose()
    }

    /// Removes the transcript saved under `id`. Missing is not an error.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the delete fails.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.db
            .query("DELETE $id")
            .bind(("id", RecordId::new(TRANSCRIPT_TABLE, id)))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error(format!("deleting transcript {id}"), error))?;
        Ok(())
    }

    /// A stream of the transcript under `id`: one item for each later save. It ends when the
    /// transcript is deleted.
    ///
    /// The stream holds its own client clone, so it outlives the handle it came from; dropping
    /// the stream ends the live query. A record that cannot be decoded arrives as an `Err` and the stream goes on.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the live query cannot start.
    pub async fn watch(
        &self,
        id: &str,
    ) -> Result<impl Stream<Item = Result<Transcript>> + Send + Unpin + use<C>> {
        let live = self
            .db
            .select::<Option<TranscriptRow>>(RecordId::new(TRANSCRIPT_TABLE, id))
            .live()
            .await
            .map_err(|error| store_error(format!("watching transcript {id}"), error))?;
        let id = id.to_owned();
        let deleted = |item: &surrealdb::Result<surrealdb::Notification<TranscriptRow>>| {
            future::ready(!matches!(item, Ok(n) if n.action == Action::Delete))
        };
        Ok(live.take_while(deleted).map(move |item| {
            item.map_err(|error| store_error(format!("watching transcript {id}"), error))
                .and_then(|notification| notification.data.into_transcript(&id))
        }))
    }
}

/// A Message Batch that was submitted and has not been forgotten.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TrackedBatch {
    /// The batch id Anthropic gave.
    pub batch_id: String,
    /// The caller's name for the batch.
    pub label: String,
    /// The status when the batch was last tracked.
    pub status: MessageBatchProcessingStatus,
    /// When Anthropic created the batch.
    pub created_at: OffsetDateTime,
    /// When processing ended, once it has.
    pub ended_at: Option<OffsetDateTime>,
}

#[derive(SurrealValue)]
struct BatchRow {
    batch_id: String,
    label: String,
    status: String,
    created_at: Datetime,
    ended_at: Option<Datetime>,
}

impl BatchRow {
    fn into_tracked(self) -> Result<TrackedBatch> {
        let status =
            serde_json::from_value(serde_json::Value::String(self.status)).map_err(|source| {
                Error::store(
                    format!("decoding the status of batch {}", self.batch_id),
                    Some(Box::new(source)),
                )
            })?;
        Ok(TrackedBatch {
            batch_id: self.batch_id,
            label: self.label,
            status,
            created_at: from_datetime(&self.created_at),
            ended_at: self.ended_at.as_ref().map(from_datetime),
        })
    }
}

/// Remembers the Message Batches you submitted, so their ids survive a restart and you can wait
/// on them again.
#[derive(Debug, Clone)]
pub struct Batches<C: Connection> {
    db: Surreal<C>,
}

impl<C: Connection> Batches<C> {
    /// Defines the batch table if it is missing and keeps `db`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the schema cannot be defined.
    pub async fn try_new(db: Surreal<C>) -> Result<Self> {
        db.query(
            "DEFINE TABLE IF NOT EXISTS topcoat_batch TYPE NORMAL SCHEMAFULL;
             DEFINE FIELD IF NOT EXISTS label ON topcoat_batch TYPE string;
             DEFINE FIELD IF NOT EXISTS status ON topcoat_batch TYPE string;
             DEFINE FIELD IF NOT EXISTS created_at ON topcoat_batch TYPE datetime;
             DEFINE FIELD IF NOT EXISTS ended_at ON topcoat_batch TYPE option<datetime>;
             DEFINE FIELD IF NOT EXISTS updated_at ON topcoat_batch TYPE datetime;",
        )
        .await
        .and_then(|response| response.check())
        .map_err(|error| store_error("defining the batch table", error))?;
        Ok(Self { db })
    }

    /// Records `batch` under `label`, or updates its status if it is already tracked.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the write fails.
    pub async fn track(&self, batch: &MessageBatch, label: &str) -> Result<()> {
        let status = serde_json::to_value(batch.processing_status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        self.db
            .query(
                "UPSERT $id SET label = $label, status = $status, created_at = $created_at, \
                 ended_at = $ended_at, updated_at = time::now()",
            )
            .bind(("id", RecordId::new(BATCH_TABLE, batch.id.as_str())))
            .bind(("label", label.to_owned()))
            .bind(("status", status))
            .bind(("created_at", to_datetime(batch.created_at)))
            .bind(("ended_at", batch.ended_at.map(to_datetime)))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error(format!("tracking batch {}", batch.id), error))?;
        Ok(())
    }

    /// Every tracked batch that has not ended, oldest first.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the read fails.
    pub async fn pending(&self) -> Result<Vec<TrackedBatch>> {
        let mut response = self
            .db
            .query(
                "SELECT record::id(id) AS batch_id, label, status, created_at, ended_at \
                 FROM topcoat_batch WHERE status != 'ended' ORDER BY created_at",
            )
            .await
            .map_err(|error| store_error("listing pending batches", error))?;
        let rows: Vec<BatchRow> = response
            .take(0)
            .map_err(|error| store_error("listing pending batches", error))?;
        rows.into_iter().map(BatchRow::into_tracked).collect()
    }

    /// Stops tracking `batch_id`. Missing is not an error.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the delete fails.
    pub async fn forget(&self, batch_id: &str) -> Result<()> {
        self.db
            .query("DELETE $id")
            .bind(("id", RecordId::new(BATCH_TABLE, batch_id)))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error(format!("forgetting batch {batch_id}"), error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::surreal::test_db::temp_db;

    fn message(id: &str, text: &str) -> Message {
        serde_json::from_value(json!({
            "id": id,
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "model": "claude-sonnet-4-5",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 1, "output_tokens": 2}
        }))
        .unwrap()
    }

    fn run(text: &str) -> ToolRun {
        let reply = message("msg_1", text);
        ToolRun {
            message: reply.clone(),
            responses: vec![reply],
            messages: vec![MessageParam::user("hello")],
        }
    }

    fn batch(id: &str, status: &str, created: i64, ended: Option<&str>) -> MessageBatch {
        serde_json::from_value(json!({
            "id": id,
            "type": "message_batch",
            "processing_status": status,
            "request_counts": {
                "processing": 0, "succeeded": 0, "errored": 0, "canceled": 0, "expired": 0
            },
            "ended_at": ended,
            "created_at": format!("2026-10-{created:02}T00:00:00Z"),
            "expires_at": "2026-11-30T00:00:00Z",
            "cancel_initiated_at": null,
            "results_url": null
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let store = Transcripts::try_new(temp_db().await).await.unwrap();
        let saved = run("hi there");
        store.save("a", &saved).await.unwrap();
        let loaded = store.load("a").await.unwrap().unwrap();
        assert_eq!(loaded.id, "a");
        assert_eq!(loaded.messages, saved.messages);
        assert_eq!(loaded.responses, saved.responses);
        assert_eq!(loaded.error, None);
    }

    #[tokio::test]
    async fn save_failed_keeps_the_error_text() {
        let store = Transcripts::try_new(temp_db().await).await.unwrap();
        let error = Error::store("boom", None);
        let expected = error.to_string();
        let mut failed = ToolRunError {
            error,
            responses: Vec::new(),
            messages: vec![MessageParam::user("hello")],
        };
        failed.responses.push(message("msg_2", "partial"));
        store.save_failed("b", &failed).await.unwrap();
        let loaded = store.load("b").await.unwrap().unwrap();
        assert_eq!(loaded.error, Some(expected));
        assert_eq!(loaded.responses, failed.responses);
    }

    #[tokio::test]
    async fn load_of_an_unknown_id_is_none() {
        let store = Transcripts::try_new(temp_db().await).await.unwrap();
        assert_eq!(store.load("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_removes_the_transcript() {
        let store = Transcripts::try_new(temp_db().await).await.unwrap();
        store.save("c", &run("x")).await.unwrap();
        store.delete("c").await.unwrap();
        assert_eq!(store.load("c").await.unwrap(), None);
    }

    #[tokio::test]
    async fn watch_sees_updates_and_ends_on_delete() {
        let store = Transcripts::try_new(temp_db().await).await.unwrap();
        store.save("w", &run("first")).await.unwrap();
        let mut stream = store.watch("w").await.unwrap();
        store.save("w", &run("second")).await.unwrap();
        let next = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(next.responses, run("second").responses);
        store.delete("w").await.unwrap();
        let end = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap();
        assert!(end.is_none());
    }

    #[tokio::test]
    async fn pending_skips_ended_batches() {
        let batches = Batches::try_new(temp_db().await).await.unwrap();
        batches
            .track(&batch("msgbatch_2", "in_progress", 2, None), "later")
            .await
            .unwrap();
        batches
            .track(&batch("msgbatch_1", "in_progress", 1, None), "first")
            .await
            .unwrap();
        batches
            .track(
                &batch("msgbatch_3", "ended", 3, Some("2026-10-04T00:00:00Z")),
                "done",
            )
            .await
            .unwrap();
        let pending = batches.pending().await.unwrap();
        let ids: Vec<_> = pending.iter().map(|b| b.batch_id.as_str()).collect();
        assert_eq!(ids, ["msgbatch_1", "msgbatch_2"]);
        assert_eq!(pending[0].label, "first");
        assert_eq!(pending[0].status, MessageBatchProcessingStatus::InProgress);
        assert_eq!(pending[0].ended_at, None);
    }

    #[tokio::test]
    async fn tracking_twice_updates_the_status() {
        let batches = Batches::try_new(temp_db().await).await.unwrap();
        batches
            .track(&batch("msgbatch_1", "in_progress", 1, None), "job")
            .await
            .unwrap();
        batches
            .track(&batch("msgbatch_1", "canceling", 1, None), "job")
            .await
            .unwrap();
        let pending = batches.pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, MessageBatchProcessingStatus::Canceling);
    }

    #[tokio::test]
    async fn forget_removes_the_batch() {
        let batches = Batches::try_new(temp_db().await).await.unwrap();
        batches
            .track(&batch("msgbatch_1", "in_progress", 1, None), "job")
            .await
            .unwrap();
        batches.forget("msgbatch_1").await.unwrap();
        assert!(batches.pending().await.unwrap().is_empty());
    }
}
