//! The `surreal` pack: read-only SurrealQL over the app's own database.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use surrealdb::method::Transaction;
use surrealdb::types::Value as SurrealValue;
use surrealdb::{Connection, Surreal};
use tokio::runtime::Handle;
use tokio::time;

use crate::code_mode::{Example, HostError, Pack, PyError, host_fn};
use crate::surreal::{Reader, store_error};

const TIMEOUT: Duration = Duration::from_secs(20);

// The crate's own tables (memory, transcripts and the like) hold every user's data, so the pack
// neither lists nor describes them. They grant record users nothing either, so a query cannot
// read them.
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

/// `query`, `tables` and `describe` over the database of `reader`, plus the `rows`, `one` and
/// `count` helpers.
///
/// Queries run on the [`Reader`]'s session, a record user, so SurrealDB's permissions, not this
/// crate, decide what a query can read: a table is visible only if its definition grants `select`
/// to record users, and the pack cannot define, remove or alter anything. The crate's own
/// `topcoat_` tables grant nothing. As a second line of defence every query runs in a
/// transaction that is always cancelled, so nothing a table's permissions let it write stays.
///
/// The call returns after 20 s with a timeout error, but the local engine has no way to cancel
/// a query, so a CPU-bound one may run to completion in the engine. `query` returns at most
/// `max_rows` rows, and a final `{"_truncated": true, "_rows_dropped": n}` row when there were
/// more.
#[must_use]
pub fn surreal<C: Connection>(reader: &Reader<C>, max_rows: usize) -> Pack {
    let (query_db, tables_db, describe_db) = (reader.session(), reader.root(), reader.root());
    python_pack()
        .with_host(host_fn(
            "query",
            "query(*, surql: str, params: dict = {}) -> list[dict]",
            "run SurrealQL, binding $name values from params; rows of the last statement, at \
             most the row cap, then {\"_truncated\": True, \"_rows_dropped\": n} when cut; \
             never writes",
            move |args: QueryArgs| {
                let reader = query_db.clone();
                async move { query(&reader, args, max_rows).await }
            },
        ))
        .with_host(host_fn(
            "tables",
            "tables() -> list[str]",
            "the names of the tables the app has opened to this pack, sorted",
            move |_: NoArgs| {
                let db = tables_db.clone();
                async move { tables(&db).await }
            },
        ))
        .with_host(host_fn(
            "describe",
            "describe(*, table: str) -> dict",
            "the fields and indexes of a table from tables(), as SurrealDB defines them",
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
        "SurrealQL over the tables the app has opened to this pack; it never writes. Start with \
         tables() and describe(), and bind values with params ($name), never string formatting",
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
    reader: &Surreal<C>,
    args: QueryArgs,
    max_rows: usize,
) -> Result<Value, HostError> {
    let tx = Guard::begin(reader).await?;
    let outcome = time::timeout(TIMEOUT, async {
        let mut response = tx
            .get()?
            .query(&args.surql)
            .bind(Value::Object(args.params))
            .await
            .map_err(outer_error)?;
        take_last(&mut response)
    })
    .await;
    // The query's own error outranks a failed cancel.
    let value = match (outcome, tx.finish().await) {
        (Ok(Ok(value)), Ok(())) => value,
        (Ok(Ok(_)), Err(error)) => return Err(fatal("cancelling a read-only transaction", error)),
        (Ok(Err(error)), _) => return Err(error),
        (Err(_), _) => return Err(raise(PyError::RuntimeError, "query timed out after 20 s")),
    };
    let mut rows = match value {
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

/// The tables whose definition in `info` (the `INFO FOR DB` result) grants `select` to record
/// users, without the crate's own.
fn visible_tables(info: &Value) -> Vec<&str> {
    let mut names: Vec<&str> = info["tables"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, definition)| {
            !name.starts_with(OWN_TABLE_PREFIX) && definition.as_str().is_some_and(select_is_open)
        })
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort_unstable();
    names
}

/// Whether a `DEFINE TABLE` string grants `select` to record users. The engine ends the
/// definition with `PERMISSIONS NONE`, `PERMISSIONS FULL`, or clauses `FOR <actions> <NONE | FULL
/// | WHERE ...>` that group the actions sharing a permission and leave out a `delete` that is
/// `FULL`. `select` is closed exactly when `PERMISSIONS NONE` ends the definition or a `NONE`
/// clause lists it.
fn select_is_open(definition: &str) -> bool {
    let Some(start) = definition.rfind("PERMISSIONS ") else {
        return false;
    };
    let permissions = &definition[start + "PERMISSIONS ".len()..];
    if permissions == "NONE" {
        return false;
    }
    !permissions.split("FOR ").skip(1).any(|clause| {
        let mut words = clause.split(' ');
        let mut actions = Vec::new();
        for word in words.by_ref() {
            actions.push(word.trim_end_matches(','));
            if !word.ends_with(',') {
                break;
            }
        }
        words
            .next()
            .is_some_and(|permission| permission.trim_end_matches(',') == "NONE")
            && actions.contains(&"select")
    })
}

async fn info_for_db<C: Connection>(db: &Surreal<C>) -> Result<Value, HostError> {
    run(db, "INFO FOR DB").await
}

async fn tables<C: Connection>(db: &Surreal<C>) -> Result<Value, HostError> {
    let info = info_for_db(db).await?;
    Ok(json!(visible_tables(&info)))
}

async fn describe<C: Connection>(db: &Surreal<C>, table: &str) -> Result<Value, HostError> {
    if table.is_empty() || !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(raise(
            PyError::ValueError,
            format!("`{table}` is not a table name: use letters, digits and underscores"),
        ));
    }
    let info = info_for_db(db).await?;
    if !visible_tables(&info).contains(&table) {
        return Err(raise(
            PyError::ValueError,
            format!("`{table}` is not available: call tables() for the tables you can read"),
        ));
    }
    let info = run(db, &format!("INFO FOR TABLE {table}")).await?;
    Ok(json!({"fields": info["fields"], "indexes": info["indexes"]}))
}

/// Runs `surql` on the root handle and returns the last statement's result.
async fn run<C: Connection>(db: &Surreal<C>, surql: &str) -> Result<Value, HostError> {
    let mut response = db.query(surql).await.map_err(outer_error)?;
    take_last(&mut response)
}

/// The last statement's result, or the engine's message for the first statement that failed.
fn take_last(response: &mut surrealdb::IndexedResults) -> Result<Value, HostError> {
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
    use crate::client_tool::ClientTool;
    use crate::surreal::Memory;
    use crate::surreal::test_db::temp_db;
    use crate::types::ToolUseBlock;

    /// A database with `open_t`, `partial_t` and `closed_t` (three rows each, `n` = 1, 2, 3), a
    /// sequence `s` and one memory file, and the pack's reader on it.
    async fn seeded() -> (Surreal<Db>, Surreal<Db>) {
        let db = temp_db().await;
        db.query(
            "DEFINE TABLE open_t SCHEMALESS PERMISSIONS FOR select FULL; \
             DEFINE TABLE partial_t SCHEMALESS PERMISSIONS FOR select WHERE n > 1; \
             DEFINE TABLE closed_t SCHEMALESS; \
             DEFINE SEQUENCE s; \
             FOR $n IN [1, 2, 3] { \
                 CREATE open_t SET n = $n; CREATE partial_t SET n = $n; \
                 CREATE closed_t SET n = $n; \
             };",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let memory = Memory::try_new(db.clone(), "alice").await.unwrap();
        let tool_use = ToolUseBlock::new(
            "toolu_1",
            memory.name(),
            json!({"command": "create", "path": "/memories/a.md", "file_text": "secret"}),
        );
        memory.call(&tool_use).await.unwrap();
        let reader = Reader::try_new(db.clone(), "test", "test").await.unwrap();
        (db, reader.session())
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

    async fn ns(reader: &Surreal<Db>, surql: &str) -> Vec<i64> {
        let rows = query(reader, args(surql), 100).await.unwrap();
        let mut ns: Vec<i64> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["n"].as_i64().unwrap())
            .collect();
        ns.sort_unstable();
        ns
    }

    async fn root_count(db: &Surreal<Db>, table: &str) -> usize {
        let rows: Vec<SurrealValue> = db
            .query(format!("SELECT * FROM {table}"))
            .await
            .unwrap()
            .take(0)
            .unwrap();
        rows.len()
    }

    #[tokio::test]
    async fn the_tables_decide_what_the_reader_sees() {
        let (_db, reader) = seeded().await;
        assert_eq!(ns(&reader, "SELECT * FROM open_t").await, [1, 2, 3]);
        assert_eq!(ns(&reader, "SELECT * FROM partial_t").await, [2, 3]);
        assert!(ns(&reader, "SELECT * FROM closed_t").await.is_empty());
        for surql in [
            "SELECT * FROM topcoat_memory",
            "SELECT * FROM <table>'topcoat_memory'",
            "SELECT * FROM type::table('topcoat_memory')",
            "SELECT * FROM topcoat_pack_user",
            "RETURN [/'/, (SELECT * FROM topcoat_memory), /'/][1]",
        ] {
            let rows = query(&reader, args(surql), 100).await.unwrap();
            // No record comes back. A bare table name selects as the name itself, a string.
            let text = rows.to_string();
            assert!(
                !text.contains("secret") && !text.contains("alice"),
                "{surql}: {text}"
            );
        }
        let mut bound = args("SELECT * FROM type::table($t)");
        bound
            .params
            .insert("t".to_string(), json!("topcoat_memory"));
        let rows = query(&reader, bound, 100).await.unwrap();
        assert_eq!(rows, json!([]));
    }

    #[tokio::test]
    async fn the_reader_cannot_write_or_change_the_schema() {
        let (db, reader) = seeded().await;
        // `CREATE` on a table that grants no create permission returns no record.
        let created = query(&reader, args("CREATE open_t SET n = 9"), 100).await;
        assert!(
            created.map_or(true, |rows| rows == json!([])),
            "the reader created a row"
        );
        assert_eq!(root_count(&db, "open_t").await, 3);
        for surql in ["DEFINE TABLE zzz", "INFO FOR DB", "REMOVE TABLE open_t"] {
            let error = query(&reader, args(surql), 100).await.unwrap_err();
            message(&error, PyError::RuntimeError);
        }
        assert_eq!(root_count(&db, "open_t").await, 3);
        assert!(tables(&db).await.unwrap().as_array().unwrap().len() == 2);
    }

    #[tokio::test]
    async fn network_functions_are_off() {
        let (_db, reader) = seeded().await;
        let error = query(&reader, args("RETURN http::get('http://127.0.0.1:9')"), 100)
            .await
            .unwrap_err();
        message(&error, PyError::RuntimeError);
    }

    #[tokio::test]
    async fn what_the_engine_lets_a_record_user_do_beyond_reading() {
        let (_db, reader) = seeded().await;
        // The engine does not gate sequences by table permissions: the reader advances a
        // sequence (the counter is not transactional). A sequence the app defines is the one
        // thing a model query can change.
        let first = query(&reader, args("RETURN sequence::nextval('s')"), 10)
            .await
            .unwrap();
        let second = query(&reader, args("RETURN sequence::nextval('s')"), 10)
            .await
            .unwrap();
        assert_eq!(first, json!([0]));
        assert_eq!(second, json!([1]));
        // The `SLEEP` statement is denied to record users; the `sleep()` function is not, and
        // the 20 s call timeout is what bounds it.
        let error = query(&reader, args("SLEEP 10ms"), 10).await.unwrap_err();
        message(&error, PyError::RuntimeError);
        assert_eq!(
            query(&reader, args("RETURN sleep(10ms)"), 10)
                .await
                .unwrap(),
            json!([])
        );
        // A `LIVE SELECT` is accepted and returns the live query's id.
        let live = query(&reader, args("LIVE SELECT * FROM open_t"), 10)
            .await
            .unwrap();
        assert_eq!(live.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn plain_statements_and_functions_work() {
        let (_db, reader) = seeded().await;
        let var = query(&reader, args("LET $x = 1; RETURN $x"), 10)
            .await
            .unwrap();
        assert_eq!(var, json!([1]));
        let sum = query(&reader, args("RETURN math::sum([1,2])"), 10)
            .await
            .unwrap();
        assert_eq!(sum, json!([3]));
        let mut bound = args("SELECT n FROM open_t WHERE n > $min ORDER BY n");
        bound.params.insert("min".to_string(), json!(1));
        let rows = query(&reader, bound, 10).await.unwrap();
        assert_eq!(rows, json!([{"n": 2}, {"n": 3}]));
    }

    #[tokio::test]
    async fn a_syntax_error_raises_with_the_database_message() {
        let (_db, reader) = seeded().await;
        let error = query(&reader, args("SELEC * FROM open_t"), 10)
            .await
            .unwrap_err();
        assert!(!message(&error, PyError::RuntimeError).is_empty());
        let error = query(&reader, args("THROW 'boom'"), 10).await.unwrap_err();
        assert!(message(&error, PyError::RuntimeError).contains("boom"));
    }

    #[tokio::test]
    async fn rows_over_the_cap_are_cut_and_marked() {
        let (_db, reader) = seeded().await;
        let rows = query(&reader, args("SELECT n FROM open_t ORDER BY n"), 2)
            .await
            .unwrap();
        assert_eq!(
            rows,
            json!([{"n": 1}, {"n": 2}, {"_truncated": true, "_rows_dropped": 1}])
        );
    }

    #[tokio::test]
    async fn tables_lists_only_the_open_ones_and_describe_follows() {
        let (db, _reader) = seeded().await;
        assert_eq!(tables(&db).await.unwrap(), json!(["open_t", "partial_t"]));
        let described = describe(&db, "open_t").await.unwrap();
        assert!(described["fields"].is_object());
        assert!(described["indexes"].is_object());
        for hidden in ["closed_t", "topcoat_memory", "nope"] {
            let error = describe(&db, hidden).await.unwrap_err();
            message(&error, PyError::ValueError);
        }
        for bad in ["", "open_t; DEFINE TABLE x", "a-b"] {
            let error = describe(&db, bad).await.unwrap_err();
            message(&error, PyError::ValueError);
        }
    }

    #[tokio::test]
    async fn building_the_reader_twice_keeps_both_working() {
        let (db, first) = seeded().await;
        let second = Reader::try_new(db, "test", "test").await.unwrap();
        assert_eq!(
            ns(&second.session(), "SELECT * FROM open_t").await,
            [1, 2, 3]
        );
        assert_eq!(ns(&first, "SELECT * FROM open_t").await, [1, 2, 3]);
        let _pack = surreal(&second, 5);
    }

    #[tokio::test]
    async fn writes_to_a_table_that_grants_them_are_rolled_back() {
        let (db, reader) = seeded().await;
        db.query("DEFINE TABLE w SCHEMALESS PERMISSIONS FULL; CREATE w SET n = 1;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let created = query(&reader, args("CREATE w SET n = 2"), 10)
            .await
            .unwrap();
        assert_eq!(created.as_array().unwrap().len(), 1);
        assert_eq!(root_count(&db, "w").await, 1);
        query(&reader, args("DELETE w"), 10).await.unwrap();
        assert_eq!(root_count(&db, "w").await, 1);
    }

    #[tokio::test]
    async fn permission_strings_from_the_engine_are_read_correctly() {
        let db = temp_db().await;
        let cases = [
            ("", false),
            ("PERMISSIONS NONE", false),
            ("PERMISSIONS FULL", true),
            ("PERMISSIONS FOR select FULL", true),
            ("PERMISSIONS FOR select WHERE n > 1", true),
            ("PERMISSIONS FOR update FULL", false),
            ("PERMISSIONS FOR select, update FULL", true),
            ("PERMISSIONS FOR create, delete NONE", false),
            ("PERMISSIONS FOR select NONE, FOR update FULL", false),
            ("PERMISSIONS FOR select WHERE n > 1, FOR update NONE", true),
        ];
        for (index, (clause, _)) in cases.iter().enumerate() {
            db.query(format!("DEFINE TABLE c{index} SCHEMALESS {clause}"))
                .await
                .unwrap()
                .check()
                .unwrap();
        }
        let info = run(&db, "INFO FOR DB").await.unwrap();
        for (index, (clause, open)) in cases.iter().enumerate() {
            let definition = info["tables"][format!("c{index}")].as_str().unwrap();
            println!("FIXTURE {clause:?} => {definition}");
            assert_eq!(select_is_open(definition), *open, "{clause}: {definition}");
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
