//! The `surreal` pack: read-only SurrealQL over the app's own database.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use surrealdb::method::Transaction;
use surrealdb::types::ToSql;
use surrealdb::types::Value as SurrealValue;
use surrealdb::{Connection, Surreal};
use tokio::runtime::Handle;
use tokio::time;

use crate::code_mode::{Example, HostError, Pack, PyError, host_fn};
use crate::surreal::store_error;

const TIMEOUT: Duration = Duration::from_secs(20);

// The pack is read-only, and the always-cancelled transaction is only the second line of defence:
// some effects are not rolled back, and a write can fire an event or a field `VALUE` that has
// one. So the canonical form of the query is checked first, and these are refused.
//
// Statements that write or define: changing data or schema is not what a read is for, and
// transaction control, live queries, `SLEEP`, session changes and schema introspection would
// keep a write, outlive the call or hold the connection.
const FORBIDDEN_WORDS: &[&str] = &[
    "DEFINE", "REMOVE", "ALTER", "REBUILD", "BEGIN", "COMMIT", "CANCEL", "USE", "KILL", "LIVE",
    "SLEEP", "OPTION", "ACCESS", "INFO", "CREATE", "UPDATE", "UPSERT", "DELETE", "INSERT",
    "RELATE",
];

// Functions that act outside the transaction (a sequence's counter is not rolled back; HTTP, API
// and file calls reach the network and disk; `fn::` and `eval::` run arbitrary code), functions
// that build a table or record reference from a string (they would hide a `topcoat_` name), and
// `sleep`.
const FORBIDDEN_FRAGMENTS: &[&str] = &[
    "sequence::",
    "http::",
    "api::",
    "file::",
    "fn::",
    "eval::",
    "type::table(",
    "type::thing(",
    "type::record(",
    "sleep(",
    "<record>",
];

// The crate's own tables (memory, transcripts and the like) hold every user's data, so the pack
// neither lists, describes nor reads them.
const OWN_TABLE_PREFIX: &str = "topcoat_";

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
/// A query is parsed by SurrealDB's parser and refused, on its canonical form, if it has a
/// statement that writes or defines, a function that acts outside the transaction or builds a
/// table reference from a string, or a name of the crate's own `topcoat_` tables, which
/// `tables()` and `describe()` hide too. The canonical text is what runs. As a second line of
/// defence every query runs in a transaction that is always cancelled.
///
/// The call returns after 20 s with a timeout error, but the local engine has no way to cancel
/// a query, so a CPU-bound one may run to completion in the engine. `query` returns at most
/// `max_rows` rows, and a final `{"_truncated": true, "_rows_dropped": n}` row when there were
/// more.
#[must_use]
pub fn surreal<C: Connection>(db: Surreal<C>, max_rows: usize) -> Pack {
    let (tables_db, describe_db) = (db.clone(), db.clone());
    python_pack()
        .with_host(host_fn(
            "query",
            "query(*, surql: str, params: dict = {}) -> list[dict]",
            "run SurrealQL, binding $name values from params; rows of the last statement, at \
             most the row cap, then {\"_truncated\": True, \"_rows_dropped\": n} when cut; \
             read-only",
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
    let ast = surrealdb_core::syn::parse(&args.surql)
        .map_err(|error| raise(PyError::RuntimeError, error.to_string()))?;
    let canonical = ast.to_sql();
    if let Some(word) = forbidden(&canonical) {
        return Err(raise(
            PyError::ValueError,
            format!(
                "`{word}` is not allowed: the surreal pack is read-only. Use SELECT and RETURN, \
                 and bind values with params"
            ),
        ));
    }
    let mut rows = match run(db, &canonical, args.params).await? {
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
        .filter(|name| !name.starts_with(OWN_TABLE_PREFIX))
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
    if table.starts_with(OWN_TABLE_PREFIX) {
        return Err(raise(
            PyError::ValueError,
            format!(
                "`{table}` is not available: the surreal pack does not expose `topcoat_` tables"
            ),
        ));
    }
    let info = run(db, &format!("INFO FOR TABLE {table}"), Map::new()).await?;
    Ok(json!({"fields": info["fields"], "indexes": info["indexes"]}))
}

/// The first word or fragment of the canonical query `canonical` that the pack refuses. String
/// literals and backtick-quoted identifiers are blanked first, so a keyword inside them is fine.
/// The canonical form upper-cases keywords and escapes identifiers that collide with them, so an
/// unescaped upper-case word is the keyword.
fn forbidden(canonical: &str) -> Option<String> {
    let text = blank_quoted(canonical);
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if let Some(word) = text
        .split(|c: char| !is_word(c))
        .find(|word| FORBIDDEN_WORDS.contains(word))
    {
        return Some(word.to_string());
    }
    let lower = text.to_ascii_lowercase();
    if let Some(fragment) = FORBIDDEN_FRAGMENTS
        .iter()
        .find(|fragment| lower.contains(*fragment))
    {
        return Some((*fragment).to_string());
    }
    lower
        .match_indices(OWN_TABLE_PREFIX)
        .find_map(|(start, _)| {
            let on_boundary = !lower[..start].chars().next_back().is_some_and(is_word);
            on_boundary.then(|| {
                let end = lower[start..]
                    .find(|c: char| !is_word(c))
                    .map_or(lower.len(), |len| start + len);
                text[start..end].to_string()
            })
        })
}

/// `text` with each `'...'`, `"..."` and backtick-quoted run, backslash escapes included,
/// replaced by one space. A backtick-quoted name is kept (unquoted) when it is followed by `::`
/// (a function module such as `` `sequence`::nextval ``) or starts with `topcoat_`, since those
/// are names the checks look for.
fn blank_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quote = None;
    let mut inner = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match quote {
            None if matches!(c, '\'' | '"' | '`') => {
                quote = Some(c);
                inner.clear();
            }
            None => out.push(c),
            Some(_) if c == '\\' => {
                chars.next();
            }
            Some(open) if c == open => {
                quote = None;
                out.push(' ');
                if open == '`' && chars.as_str().starts_with("::") {
                    out.push_str(&inner);
                } else if open == '`' && inner.to_ascii_lowercase().starts_with(OWN_TABLE_PREFIX) {
                    out.push_str(&inner);
                    out.push(' ');
                }
            }
            Some(_) => inner.push(c),
        }
    }
    if quote.is_some() {
        out.push(' ');
    }
    out
}

/// Runs `surql` in a transaction that is always cancelled and returns the last statement's
/// result.
async fn run<C: Connection>(
    db: &Surreal<C>,
    surql: &str,
    params: Map<String, Value>,
) -> Result<Value, HostError> {
    let tx = Guard::begin(db).await?;
    let outcome = time::timeout(TIMEOUT, async {
        let mut response = tx
            .get()?
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
    match (outcome, tx.finish().await) {
        (Ok(Ok(value)), Ok(())) => Ok(value),
        (Ok(Ok(_)), Err(error)) => Err(fatal("cancelling a read-only transaction", error)),
        (Ok(Err(error)), _) => Err(error),
        (Err(_), _) => Err(raise(PyError::RuntimeError, "query timed out after 20 s")),
    }
}

/// A transaction that is cancelled however the call ends. The client `Transaction` has no `Drop`,
/// so a call future dropped mid-query (the tool loop drops its siblings on a fatal error) would
/// leave the write transaction open in the engine.
struct Guard<C: Connection>(Option<Transaction<C>>);

impl<C: Connection> Guard<C> {
    async fn begin(db: &Surreal<C>) -> Result<Self, HostError> {
        db.clone()
            .begin()
            .await
            .map(|tx| Self(Some(tx)))
            .map_err(|error| fatal("beginning a read-only transaction", error))
    }

    fn get(&self) -> Result<&Transaction<C>, HostError> {
        self.0
            .as_ref()
            .ok_or_else(|| raise(PyError::RuntimeError, "the transaction is already closed"))
    }

    async fn finish(mut self) -> Result<(), surrealdb::Error> {
        match self.0.take() {
            Some(tx) => tx.cancel().await.map(|_| ()),
            None => Ok(()),
        }
    }
}

impl<C: Connection> Drop for Guard<C> {
    fn drop(&mut self) {
        if let (Some(tx), Ok(runtime)) = (self.0.take(), Handle::try_current()) {
            runtime.spawn(async move {
                let _ = tx.cancel().await;
            });
        }
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
    async fn a_select_in_the_transaction_sees_committed_rows() {
        let db = seeded(1).await;
        let rows = query(&db, args("SELECT n FROM item:1"), 10).await.unwrap();
        assert_eq!(rows, json!([{"n": 1}]));
    }

    #[tokio::test]
    async fn text_that_writes_or_acts_outside_the_transaction_is_refused() {
        let db = seeded(1).await;
        for surql in [
            "RETURN `sequence`::nextval('s')",
            "RETURN sequence :: nextval('s')",
            "/* x */ DEFINE TABLE y",
            "RETURN http::get('http://127.0.0.1:1')",
            "RETURN fn::anything()",
            "RETURN eval::surql('RETURN 1')",
            "SELECT * FROM topcoat_memory",
            "SELECT * FROM type::table($t)",
            "SELECT * FROM <record>$r",
            "CREATE item SET n = 1",
        ] {
            let error = query(&db, args(surql), 10).await.unwrap_err();
            let text = message(&error, PyError::ValueError);
            assert!(text.contains("read-only"), "{surql}: {text}");
        }
    }

    #[tokio::test]
    async fn quoted_words_and_plain_functions_are_allowed() {
        let db = seeded(1).await;
        let rows = query(
            &db,
            args("SELECT n FROM item WHERE 'DEFINE http::' != 'x'"),
            10,
        )
        .await
        .unwrap();
        assert_eq!(rows, json!([{"n": 1}]));
        // The parser drops comments, so a keyword in one is not in the text that runs.
        let commented = query(&db, args("SELECT n FROM item -- DEFINE"), 10)
            .await
            .unwrap();
        assert_eq!(commented, json!([{"n": 1}]));
        query(&db, args("SELECT `access` FROM item"), 10)
            .await
            .unwrap();
        let sum = query(&db, args("RETURN math::sum([1,2])"), 10)
            .await
            .unwrap();
        assert_eq!(sum, json!([3]));
        let var = query(&db, args("LET $x = 1; RETURN $x"), 10).await.unwrap();
        assert_eq!(var, json!([1]));
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

    #[tokio::test]
    async fn the_crates_own_tables_are_hidden() {
        let db = seeded(1).await;
        db.query("DEFINE TABLE topcoat_x SCHEMALESS")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(tables(&db).await.unwrap(), json!(["item"]));
        let error = describe(&db, "topcoat_x").await.unwrap_err();
        message(&error, PyError::ValueError);
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
