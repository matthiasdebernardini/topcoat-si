//! The `surreal` pack: read-only SurrealQL over the app's own database.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use surrealdb::types::Value as SurrealValue;
use surrealdb::{Connection, Surreal};
use tokio::time;

use crate::code_mode::{Example, HostError, Pack, PyError, host_fn};
use crate::surreal::store_error;

const TIMEOUT: Duration = Duration::from_secs(20);

// Words the query text may not contain. The cancel at the end undoes every write, so `CREATE`,
// `UPDATE` and `DELETE` are harmless. These are not, or would defeat the cancel.
const FORBIDDEN_WORDS: &[&str] = &[
    // DDL: the build of an index and a sequence's counter act outside the transaction, and
    // changing the schema is not what a read is for.
    "define", "remove", "alter", "rebuild",
    // Ending or leaving the transaction would keep a write or run the rest outside it.
    "begin", "commit", "cancel", "use",
    // Live queries and kills outlive the call; sleep holds the connection.
    "kill", "live", "sleep",
    // Session and permission changes, and schema introspection that is not `tables()` or
    // `describe()`.
    "option", "access", "info",
];

// Function families that act outside the transaction: a sequence's counter is not rolled back,
// and HTTP, API and file calls reach the network and disk.
const FORBIDDEN_PREFIXES: &[&str] = &["sequence::", "http::", "api::", "file::"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArgs {
    surql: String,
    #[serde(default)]
    params: Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeArgs {
    table: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

/// `query`, `tables` and `describe` over `db`, plus the `rows`, `one` and `count` helpers.
///
/// Every query runs in a transaction that is always cancelled, so nothing it writes survives,
/// and text with DDL, transaction control, `sleep`, live queries or `sequence::`, `http::`,
/// `api::` or `file::` functions is refused before it runs. A query that takes over 20 s is
/// cancelled. `query` returns at most `max_rows` rows, and a final `{"_truncated": true,
/// "_rows_dropped": n}` row when there were more.
#[must_use]
pub fn surreal<C: Connection>(db: Surreal<C>, max_rows: usize) -> Pack {
    let (tables_db, describe_db) = (db.clone(), db.clone());
    python_pack()
        .with_host(host_fn(
            "query",
            "query(*, surql: str, params: dict = {}) -> list[dict]",
            "run SurrealQL, binding $name values from params; rows of the last statement, at \
             most the row cap, then {\"_truncated\": True, \"_rows_dropped\": n} when cut; \
             writes are undone",
            move |args: QueryArgs| {
                let db = db.clone();
                async move { query(&db, args, max_rows).await }
            },
        ))
        .with_host(host_fn(
            "tables",
            "tables() -> list[str]",
            "the table names, sorted",
            move |_: NoArgs| {
                let db = tables_db.clone();
                async move { tables(&db).await }
            },
        ))
        .with_host(host_fn(
            "describe",
            "describe(*, table: str) -> dict",
            "the fields and indexes of a table, as SurrealDB defines them",
            move |args: DescribeArgs| {
                let db = describe_db.clone();
                async move { describe(&db, &args.table).await }
            },
        ))
}

/// The pack without its host functions: the Python helpers and their examples, which
/// `check_pack` runs against fake host functions.
fn python_pack() -> Pack {
    Pack::new(
        "surreal",
        "read-only SurrealQL over the app's data: start with tables() and describe(), and bind \
         values with params ($name), never string formatting",
    )
    .with_source(include_str!("surreal.py"))
    .with_python_fn(
        "rows",
        "async def rows(*, surql: str, params: dict = {}) -> list[dict]",
        "query without the truncation marker row",
    )
    .with_python_fn(
        "one",
        "async def one(*, surql: str, params: dict = {}) -> dict | None",
        "the first row of a query, or None",
    )
    .with_python_fn(
        "count",
        "async def count(*, table: str) -> int",
        "how many records a table holds",
    )
    .with_example(
        "rows",
        Example {
            code: r#"await rows(surql="SELECT * FROM item WHERE n > $n", params={"n": 0})"#,
            expect: "=> [{'id': 'item:1', 'n': 1}, {'id': 'item:2', 'n': 2}]",
        },
    )
    .with_example(
        "one",
        Example {
            code: r#"await one(surql="SELECT * FROM item WHERE n > $n", params={"n": 0})"#,
            expect: "=> {'id': 'item:1', 'n': 1}",
        },
    )
    .with_example(
        "count",
        Example {
            code: r#"await count(table="item")"#,
            expect: "=> 2",
        },
    )
}

async fn query<C: Connection>(
    db: &Surreal<C>,
    args: QueryArgs,
    max_rows: usize,
) -> Result<Value, HostError> {
    if let Some(word) = forbidden(&args.surql) {
        return Err(raise(
            PyError::ValueError,
            format!(
                "`{word}` is not allowed: the surreal pack is read-only. Use SELECT and RETURN, \
                 and bind values with params"
            ),
        ));
    }
    let mut rows = match run(db, &args.surql, args.params).await? {
        Value::Array(rows) => rows,
        Value::Null => Vec::new(),
        row => vec![row],
    };
    if rows.len() > max_rows {
        let dropped = rows.len() - max_rows;
        rows.truncate(max_rows);
        rows.push(json!({"_truncated": true, "_rows_dropped": dropped}));
    }
    Ok(Value::Array(rows))
}

async fn tables<C: Connection>(db: &Surreal<C>) -> Result<Value, HostError> {
    let info = run(db, "INFO FOR DB", Map::new()).await?;
    let mut names: Vec<&String> = info["tables"]
        .as_object()
        .into_iter()
        .flat_map(Map::keys)
        .collect();
    names.sort();
    Ok(json!(names))
}

async fn describe<C: Connection>(db: &Surreal<C>, table: &str) -> Result<Value, HostError> {
    if table.is_empty() || !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(raise(
            PyError::ValueError,
            format!("`{table}` is not a table name: use letters, digits and underscores"),
        ));
    }
    let info = run(db, &format!("INFO FOR TABLE {table}"), Map::new()).await?;
    Ok(json!({"fields": info["fields"], "indexes": info["indexes"]}))
}

/// The first word or function prefix of `surql` that the pack refuses, as written in the text.
/// A word is a run of letters, digits, `_` and `:`, so `a::use` and `defined` are fine.
fn forbidden(surql: &str) -> Option<&str> {
    surql
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'))
        .find(|word| {
            let lower = word.to_ascii_lowercase();
            FORBIDDEN_WORDS.contains(&lower.as_str())
                || FORBIDDEN_PREFIXES
                    .iter()
                    .any(|prefix| lower.starts_with(prefix))
        })
}

/// Runs `surql` in a transaction that is always cancelled and returns the last statement's
/// result.
async fn run<C: Connection>(
    db: &Surreal<C>,
    surql: &str,
    params: Map<String, Value>,
) -> Result<Value, HostError> {
    let tx = db
        .clone()
        .begin()
        .await
        .map_err(|error| fatal("beginning a read-only transaction", error))?;
    let outcome = time::timeout(TIMEOUT, async {
        let mut response = tx
            .query(surql)
            .bind(Value::Object(params))
            .await
            .map_err(outer_error)?;
        // Several statements can fail; the first one is the cause, the rest "not executed".
        let errors = response.take_errors();
        if let Some((_, error)) = errors.iter().min_by_key(|(index, _)| **index) {
            return Err(raise(PyError::RuntimeError, error.message()));
        }
        match response.num_statements().checked_sub(1) {
            Some(last) => response
                .take::<SurrealValue>(last)
                .map(SurrealValue::into_json_value)
                .map_err(|error| raise(PyError::RuntimeError, error.message())),
            None => Ok(Value::Null),
        }
    })
    .await;
    // The query's own error outranks a failed cancel.
    match (outcome, tx.cancel().await) {
        (Ok(Ok(value)), Ok(_)) => Ok(value),
        (Ok(Ok(_)), Err(error)) => Err(fatal("cancelling a read-only transaction", error)),
        (Ok(Err(error)), _) => Err(error),
        (Err(_), _) => Err(raise(PyError::RuntimeError, "query timed out after 20 s")),
    }
}

/// A failure of the call itself: the query's fault raises, the database's ends the run.
fn outer_error(error: surrealdb::Error) -> HostError {
    if error.is_connection() || error.is_internal() || error.is_configuration() {
        fatal("running a read-only query", error)
    } else {
        raise(PyError::RuntimeError, error.message())
    }
}

fn fatal(context: &str, error: surrealdb::Error) -> HostError {
    HostError::Fatal(store_error(context, error))
}

fn raise(kind: PyError, message: impl Into<String>) -> HostError {
    HostError::Raise {
        kind,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use surrealdb::engine::local::Db;

    use super::*;
    use crate::surreal::test_db::temp_db;

    async fn seeded(rows: u32) -> Surreal<Db> {
        let db = temp_db().await;
        db.query("DEFINE TABLE item SCHEMALESS; DEFINE FIELD n ON item TYPE int")
            .await
            .unwrap()
            .check()
            .unwrap();
        for n in 1..=rows {
            db.query(format!("CREATE item:{n} SET n = {n}"))
                .await
                .unwrap()
                .check()
                .unwrap();
        }
        db
    }

    fn args(surql: &str) -> QueryArgs {
        QueryArgs {
            surql: surql.to_string(),
            params: Map::new(),
        }
    }

    fn message(error: &HostError, expected: PyError) -> String {
        match error {
            HostError::Raise { kind, message } if *kind == expected => message.clone(),
            other => panic!("expected {expected:?}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn select_returns_the_rows_and_binds_params() {
        let db = seeded(3).await;
        let mut bound = args("SELECT n FROM item WHERE n > $min ORDER BY n");
        bound.params.insert("min".to_string(), json!(1));
        let rows = query(&db, bound, 10).await.unwrap();
        assert_eq!(rows, json!([{"n": 2}, {"n": 3}]));
    }

    #[tokio::test]
    async fn a_write_succeeds_but_does_not_survive() {
        let db = seeded(1).await;
        let created = query(&db, args("CREATE item:99 SET n = 99"), 10)
            .await
            .unwrap();
        assert_eq!(created[0]["n"], json!(99));
        let mut after = db.query("SELECT * FROM item:99").await.unwrap();
        assert_eq!(after.take::<Vec<Value>>(0).unwrap(), Vec::<Value>::new());
    }

    #[tokio::test]
    async fn text_that_acts_outside_the_transaction_is_refused() {
        let db = seeded(1).await;
        for (surql, word) in [
            ("DEFINE TABLE x", "DEFINE"),
            ("select 1; sleep(1s)", "sleep"),
            ("RETURN sequence::nextval('a')", "sequence::nextval"),
            ("RETURN http::get('http://localhost')", "http::get"),
            ("COMMIT", "COMMIT"),
            ("INFO FOR DB", "INFO"),
        ] {
            let error = query(&db, args(surql), 10).await.unwrap_err();
            let text = message(&error, PyError::ValueError);
            assert!(text.contains(word) && text.contains("read-only"), "{text}");
        }
        // Words inside longer words and paths are fine.
        assert!(forbidden("SELECT defined, a::use FROM item").is_none());
    }

    #[tokio::test]
    async fn a_syntax_error_raises_with_the_database_message() {
        let db = seeded(1).await;
        let error = query(&db, args("SELEC * FROM item"), 10).await.unwrap_err();
        assert!(!message(&error, PyError::RuntimeError).is_empty());
        let error = query(&db, args("THROW 'boom'"), 10).await.unwrap_err();
        assert!(message(&error, PyError::RuntimeError).contains("boom"));
    }

    #[tokio::test]
    async fn rows_over_the_cap_are_cut_and_marked() {
        let db = seeded(5).await;
        let rows = query(&db, args("SELECT n FROM item ORDER BY n"), 2)
            .await
            .unwrap();
        assert_eq!(
            rows,
            json!([{"n": 1}, {"n": 2}, {"_truncated": true, "_rows_dropped": 3}])
        );
    }

    #[tokio::test]
    async fn tables_and_describe_read_the_schema() {
        let db = seeded(1).await;
        assert_eq!(tables(&db).await.unwrap(), json!(["item"]));
        let described = describe(&db, "item").await.unwrap();
        assert!(described["fields"]["n"].is_string());
        assert!(described["indexes"].is_object());
        for bad in ["", "item; DEFINE TABLE x", "a-b"] {
            let error = describe(&db, bad).await.unwrap_err();
            message(&error, PyError::ValueError);
        }
    }

    fn fake_surreal() -> Pack {
        Pack::new("surreal_fake", "fake").with_host(host_fn(
            "query",
            "query(*, surql: str, params: dict = {}) -> list[dict]",
            "fake",
            |args: QueryArgs| async move {
                if args.surql.contains("count()") {
                    return Ok(json!([{"count": 2}]));
                }
                Ok(json!([
                    {"id": "item:1", "n": 1},
                    {"id": "item:2", "n": 2},
                    {"_truncated": true, "_rows_dropped": 7}
                ]))
            },
        ))
    }

    #[tokio::test]
    async fn python_examples_pass() {
        let Ok(monty) = std::env::var("MONTY_BIN") else {
            return;
        };
        let pool = crate::code_mode::pool(monty, 2).await.unwrap();
        crate::code_mode::testing::check_pack(pool, python_pack, || vec![fake_surreal()])
            .await
            .unwrap();
    }
}
