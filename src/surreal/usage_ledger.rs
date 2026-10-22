//! A durable usage ledger: one row per response, the stored version of the `topcoat_si::usage`
//! tracing event.

use surrealdb::types::SurrealValue;
use surrealdb::{Connection, Surreal};
use time::OffsetDateTime;

use super::store_error;
use super::time::to_datetime;
use crate::error::Result;
use crate::tool_loop::ToolRun;
use crate::types::{Message, Model, Usage};

const SCHEMA: &str = "
DEFINE TABLE IF NOT EXISTS topcoat_usage TYPE NORMAL SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS message_id ON topcoat_usage TYPE string;
DEFINE FIELD IF NOT EXISTS model ON topcoat_usage TYPE string;
DEFINE FIELD IF NOT EXISTS label ON topcoat_usage TYPE option<string>;
DEFINE FIELD IF NOT EXISTS input_tokens ON topcoat_usage TYPE int;
DEFINE FIELD IF NOT EXISTS output_tokens ON topcoat_usage TYPE int;
DEFINE FIELD IF NOT EXISTS cache_read_tokens ON topcoat_usage TYPE int;
DEFINE FIELD IF NOT EXISTS cache_write_tokens ON topcoat_usage TYPE int;
DEFINE FIELD IF NOT EXISTS cost_micro_cents ON topcoat_usage TYPE int;
DEFINE FIELD IF NOT EXISTS at ON topcoat_usage TYPE datetime;
DEFINE INDEX IF NOT EXISTS topcoat_usage_at ON topcoat_usage FIELDS at;
DEFINE INDEX IF NOT EXISTS topcoat_usage_model ON topcoat_usage FIELDS model;
";

const RECORD: &str = "
UPSERT type::record('topcoat_usage', $message_id) SET
    message_id = $message_id,
    model = $model,
    label = $label,
    input_tokens = $input_tokens,
    output_tokens = $output_tokens,
    cache_read_tokens = $cache_read_tokens,
    cache_write_tokens = $cache_write_tokens,
    cost_micro_cents = $cost_micro_cents,
    at = at ?? time::now();
";

const TOTALS: &str = "
SELECT
    model,
    count() AS calls,
    math::sum(input_tokens) AS input_tokens,
    math::sum(output_tokens) AS output_tokens,
    math::sum(cache_read_tokens) AS cache_read_tokens,
    math::sum(cache_write_tokens) AS cache_write_tokens,
    math::sum(cost_micro_cents) AS cost_micro_cents
FROM topcoat_usage
WHERE at >= $since
GROUP BY model
ORDER BY model;
";

/// What one model used since a point in time.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ModelTotal {
    /// The model name as the API returned it.
    pub model: String,
    /// Responses recorded.
    pub calls: u64,
    /// Input tokens, not counting cache reads and writes.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Input tokens served from the prompt cache.
    pub cache_read_tokens: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write_tokens: u64,
    /// Cost in micro-cents, as [`KnownModel::cost_micro_cents`](crate::KnownModel::cost_micro_cents)
    /// prices it; zero for a custom model, which has no built-in pricing.
    pub cost_micro_cents: u64,
}

/// The cost of `usage` on `model`; zero for a custom model, which has no built-in pricing.
fn cost_of(model: &Model, usage: &Usage) -> u64 {
    match model {
        Model::Known(known) => known.cost_micro_cents(usage),
        Model::Custom(_) => 0,
    }
}

/// The shape of a [`TOTALS`] row.
#[derive(SurrealValue)]
struct TotalRow {
    model: String,
    calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cost_micro_cents: i64,
}

impl From<TotalRow> for ModelTotal {
    fn from(row: TotalRow) -> Self {
        let count = |n: i64| u64::try_from(n).unwrap_or(0);
        Self {
            model: row.model,
            calls: count(row.calls),
            input_tokens: count(row.input_tokens),
            output_tokens: count(row.output_tokens),
            cache_read_tokens: count(row.cache_read_tokens),
            cache_write_tokens: count(row.cache_write_tokens),
            cost_micro_cents: count(row.cost_micro_cents),
        }
    }
}

/// Records every response's usage and cost, keyed by message id, and sums them per model.
///
/// Recording the same response twice leaves one row, so a retry cannot double count.
#[derive(Debug, Clone)]
pub struct UsageLedger<C: Connection> {
    db: Surreal<C>,
}

impl<C: Connection> UsageLedger<C> {
    /// Defines the `topcoat_usage` table, if it is not there yet, on `db`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the schema cannot be applied.
    pub async fn try_new(db: Surreal<C>) -> Result<Self> {
        db.query(SCHEMA)
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error("defining the usage ledger", error))?;
        Ok(Self { db })
    }

    /// Records one response, with an optional `label` such as the feature that made the call.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the write fails.
    pub async fn record(&self, message: &Message, label: Option<&str>) -> Result<()> {
        let usage = &message.usage;
        // Counts are never negative in practice; clamp so a bad value cannot break a sum.
        let tokens = |n: i32| i64::from(n.max(0));
        let cost = cost_of(&message.model, usage);
        self.db
            .query(RECORD)
            .bind(("message_id", message.id.clone()))
            .bind(("model", message.model.to_string()))
            .bind(("label", label.map(str::to_owned)))
            .bind(("input_tokens", tokens(usage.input_tokens)))
            .bind(("output_tokens", tokens(usage.output_tokens)))
            .bind((
                "cache_read_tokens",
                tokens(usage.cache_read_input_tokens.unwrap_or(0)),
            ))
            .bind((
                "cache_write_tokens",
                tokens(usage.cache_creation_input_tokens.unwrap_or(0)),
            ))
            .bind(("cost_micro_cents", i64::try_from(cost).unwrap_or(i64::MAX)))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error(format!("recording usage of {}", message.id), error))?;
        Ok(())
    }

    /// Records every response of `run`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when a write fails; responses before it
    /// stay recorded.
    pub async fn record_run(&self, run: &ToolRun, label: Option<&str>) -> Result<()> {
        for message in &run.responses {
            self.record(message, label).await?;
        }
        Ok(())
    }

    /// Totals per model, sorted by model name, over the rows recorded at or after `since`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the read fails.
    pub async fn totals(&self, since: OffsetDateTime) -> Result<Vec<ModelTotal>> {
        let rows: Vec<TotalRow> = self
            .db
            .query(TOTALS)
            .bind(("since", to_datetime(since)))
            .await
            .and_then(|mut response| response.take(0))
            .map_err(|error| store_error("summing the usage ledger", error))?;
        Ok(rows.into_iter().map(ModelTotal::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::surreal::test_db::temp_db;

    fn message(id: &str, model: &str, input: i32, output: i32) -> Message {
        serde_json::from_value(json!({
            "id": id,
            "content": [],
            "model": model,
            "role": "assistant",
            "type": "message",
            "usage": {"input_tokens": input, "output_tokens": output},
        }))
        .expect("minimal message")
    }

    #[tokio::test]
    async fn sums_per_model_and_counts_a_response_once() {
        let ledger = UsageLedger::try_new(temp_db().await).await.expect("schema");
        let sonnet = message("msg_1", "claude-sonnet-5-5", 100, 10);
        let haiku = message("msg_2", "claude-haiku-5-5", 7, 3);
        ledger.record(&sonnet, Some("test")).await.expect("record");
        ledger.record(&haiku, None).await.expect("record");
        ledger.record(&sonnet, Some("test")).await.expect("again");

        let totals = ledger
            .totals(OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("totals");
        assert_eq!(totals.len(), 2);
        assert_eq!(totals[0].model, "claude-haiku-5-5");
        assert_eq!(totals[0].calls, 1);
        assert_eq!((totals[0].input_tokens, totals[0].output_tokens), (7, 3));
        assert_eq!(totals[1].model, "claude-sonnet-5-5");
        assert_eq!(totals[1].calls, 1);
        assert_eq!((totals[1].input_tokens, totals[1].output_tokens), (100, 10));
        assert_eq!(
            totals[1].cost_micro_cents,
            cost_of(&sonnet.model, &sonnet.usage)
        );

        let later = OffsetDateTime::now_utc() + Duration::from_secs(3600);
        assert!(ledger.totals(later).await.expect("totals").is_empty());
    }

    #[tokio::test]
    async fn recording_again_keeps_the_first_timestamp() {
        let db = temp_db().await;
        let ledger = UsageLedger::try_new(db.clone()).await.expect("schema");
        let sonnet = message("msg_1", "claude-sonnet-5-5", 100, 10);
        let read_at = || async {
            let at: Vec<surrealdb::types::Datetime> = db
                .query("SELECT VALUE at FROM topcoat_usage")
                .await
                .and_then(|mut response| response.take(0))
                .expect("read at");
            at
        };
        ledger.record(&sonnet, None).await.expect("record");
        let first = read_at().await;
        assert_eq!(first.len(), 1);
        tokio::time::sleep(Duration::from_millis(20)).await;
        ledger.record(&sonnet, None).await.expect("again");
        assert_eq!(read_at().await, first);
    }
}
