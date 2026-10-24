//! The eval results in SurrealDB: every `RunRecord` of every run, to query without a JSONL file.

use std::path::Path;

use serde_json::Value;
use carpa::surrealdb::Surreal;
use carpa::surrealdb::engine::local::Db;

use crate::run::RunRecord;

const SCHEMA: &str = "
DEFINE TABLE IF NOT EXISTS eval_run SCHEMALESS;
DEFINE INDEX IF NOT EXISTS eval_run_run ON eval_run FIELDS run;
";

/// A SurrealKV database holding one `eval_run` row per `RunRecord`: its JSON, plus the `run`
/// stamp it belongs to and the `at` time it was stored.
pub struct EvalDb {
    db: Surreal<Db>,
}

impl EvalDb {
    /// Opens, or creates, the database in the directory `path`. SurrealKV locks the directory, so
    /// one process at a time can have it open.
    pub async fn open(path: &Path) -> Result<Self, String> {
        let db = carpa::surreal::open(path, "carpa_evals", "results")
            .await
            .map_err(|error| format!("{}: {error}", path.display()))?;
        db.query(SCHEMA)
            .await
            .and_then(|response| response.check())
            .map_err(|error| format!("{}: defining eval_run: {error}", path.display()))?;
        Ok(Self { db })
    }

    /// Stores `record` under the run stamp `run`.
    pub async fn insert(&self, run: &str, record: &RunRecord) -> Result<(), String> {
        let Value::Object(mut fields) =
            serde_json::to_value(record).map_err(|error| format!("record: {error}"))?
        else {
            return Err("record: not a JSON object".to_string());
        };
        fields.insert("run".to_string(), Value::String(run.to_string()));
        self.db
            .query("CREATE eval_run CONTENT object::extend($fields, { at: time::now() })")
            .bind(("fields", Value::Object(fields)))
            .await
            .and_then(|response| response.check())
            .map_err(|error| format!("storing a record of run {run}: {error}"))?;
        Ok(())
    }

    /// The records stored under `run`, in the order they were stored.
    pub async fn records(&self, run: &str) -> Result<Vec<RunRecord>, String> {
        let rows: Vec<Value> = self
            .db
            .query("SELECT * OMIT id, run, at FROM eval_run WHERE run = $run ORDER BY at")
            .bind(("run", run.to_string()))
            .await
            .and_then(|mut response| response.take(0))
            .map_err(|error| format!("reading run {run}: {error}"))?;
        rows.into_iter()
            .map(|row| serde_json::from_value(row).map_err(|error| format!("run {run}: {error}")))
            .collect()
    }

    /// Every run stamp stored, sorted.
    pub async fn runs(&self) -> Result<Vec<String>, String> {
        self.db
            .query("RETURN array::sort(array::distinct(SELECT VALUE run FROM eval_run))")
            .await
            .and_then(|mut response| response.take(0))
            .map_err(|error| format!("listing runs: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::cases::{Kind, Split};
    use crate::plan::Arm;

    fn record(case_id: &str, repeat: u32) -> RunRecord {
        RunRecord {
            case_id: case_id.to_string(),
            kind: Kind::Fanout,
            split: Split::Dev,
            arm: Arm::CodeMode,
            model: "claude-sonnet-5-5".to_string(),
            repeat,
            pass: true,
            deterministic_pass: true,
            judge_probability: Some(0.95),
            judge_error: None,
            answer: Some("2015, 2018".to_string()),
            error: None,
            error_kind: None,
            model_calls: 2,
            tool_calls: BTreeMap::from([("python".to_string(), 1)]),
            python_calls: 1,
            empty_final_answer: false,
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_write_tokens: 40,
            cost_micro_cents: 1234,
            wall_seconds: 2.0,
        }
    }

    #[tokio::test]
    async fn stores_records_by_run_stamp() {
        let dir = std::env::temp_dir().join(format!("carpa-evals-db-{}", std::process::id()));
        let db = EvalDb::open(&dir).await.expect("open");
        let mut failed = record("b", 1);
        failed.pass = false;
        failed.judge_probability = None;
        failed.answer = None;
        failed.error = Some("connection reset".to_string());
        let first = [record("a", 0), failed];
        for record in &first {
            db.insert("20261022T010203Z", record).await.expect("insert");
        }
        let other = record("c", 0);
        db.insert("20261023T010203Z", &other).await.expect("insert");

        assert_eq!(
            db.records("20261022T010203Z").await.expect("records"),
            first
        );
        assert_eq!(
            db.records("20261023T010203Z").await.expect("records"),
            [other]
        );
        assert!(db.records("none").await.expect("records").is_empty());
        assert_eq!(
            db.runs().await.expect("runs"),
            ["20261022T010203Z", "20261023T010203Z"]
        );
    }
}
