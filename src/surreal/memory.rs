//! Claude's memory tool on SurrealDB, plus ranked full-text search over what it stored.
//!
//! Files live as rows in `carpa_memory`, one per path and scope. Directories are implicit: a
//! directory exists while some file path has it as a prefix.

use std::collections::BTreeSet;

use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;
use surrealdb::types::SurrealValue;
use surrealdb::{Connection, Surreal};
use time::OffsetDateTime;

use super::store_error;
use super::time::to_datetime;
use crate::client_tool::ClientTool;
use crate::error::{Error, Result};
use crate::types::{MemoryTool, ToolParam, ToolResultBlock, ToolUnionParam, ToolUseBlock};

const ROOT: &str = "/memories";
const SEARCH_TOOL_NAME: &str = "memory_search";
const MAX_SEARCH_LIMIT: usize = 50;
const MAX_TOOL_LIMIT: u64 = 20;
const DEFAULT_TOOL_LIMIT: usize = 5;

const SCHEMA: &str = "
DEFINE TABLE IF NOT EXISTS carpa_memory TYPE NORMAL SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS scope ON carpa_memory TYPE string;
DEFINE FIELD IF NOT EXISTS path ON carpa_memory TYPE string;
DEFINE FIELD IF NOT EXISTS text ON carpa_memory TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON carpa_memory TYPE datetime;
DEFINE INDEX IF NOT EXISTS carpa_memory_scope_path ON carpa_memory FIELDS scope, path UNIQUE;
DEFINE ANALYZER IF NOT EXISTS carpa_english TOKENIZERS blank, class
    FILTERS lowercase, ascii, snowball(english);
DEFINE INDEX IF NOT EXISTS carpa_memory_text ON carpa_memory FIELDS text
    FULLTEXT ANALYZER carpa_english BM25;
";

/// Claude's `memory` tool, kept in SurrealDB.
///
/// Each value works on one `scope`, such as a user or a conversation, and two scopes never see
/// each other's files. Problems the model can fix (a bad path, text that is not there) come back
/// as an `is_error` result. A database failure is returned as `Err` with
/// [`ErrorKind::Store`](crate::ErrorKind::Store), because only the app can fix it.
#[derive(Debug, Clone)]
pub struct Memory<C: Connection> {
    db: Surreal<C>,
    scope: String,
}

/// One file that matched a [`Memory::search`], with its BM25 relevance.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MemoryHit {
    /// The file's path, starting with `/memories`.
    pub path: String,
    /// The file's whole text.
    pub text: String,
    /// BM25 relevance; higher is a better match, and only the order means anything.
    pub score: f64,
}

/// A second tool, `memory_search`, that ranks a scope's memory files for a query.
///
/// Get one from [`Memory::search_tool`]. It shares the database and scope of the `Memory` it
/// came from.
#[derive(Debug, Clone)]
pub struct MemorySearch<C: Connection> {
    db: Surreal<C>,
    scope: String,
}

#[derive(SurrealValue)]
struct HitRow {
    path: String,
    text: String,
    score: f64,
}

/// Why a command did not produce a reply.
enum Failure {
    /// The model can fix this; it goes back as an `is_error` result.
    Model(String),
    /// Only the app can fix this; it stops the tool loop.
    Store(Error),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Store(error)
    }
}

fn model_error(message: impl Into<String>) -> Failure {
    Failure::Model(message.into())
}

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum Command {
    View {
        path: String,
        view_range: Option<[i64; 2]>,
    },
    Create {
        path: String,
        file_text: String,
    },
    StrReplace {
        path: String,
        old_str: String,
        #[serde(default)]
        new_str: String,
    },
    Insert {
        path: String,
        insert_line: i64,
        insert_text: String,
    },
    Delete {
        path: String,
    },
    Rename {
        old_path: String,
        new_path: String,
    },
}

impl<C: Connection> Memory<C> {
    /// Defines the table, analyzer and indexes if they are missing, and returns a memory for
    /// `scope`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the schema cannot be applied.
    pub async fn try_new(db: Surreal<C>, scope: impl Into<String>) -> Result<Self> {
        db.query(SCHEMA)
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error("defining the memory schema", error))?;
        Ok(Self {
            db,
            scope: scope.into(),
        })
    }

    /// The files in this scope that best match `query`, best first.
    ///
    /// `limit` is clamped to 1 through 50.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Store`](crate::ErrorKind::Store) when the query fails.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryHit>> {
        search(&self.db, &self.scope, query, limit).await
    }

    /// The `memory_search` tool over this database and scope.
    #[must_use]
    pub fn search_tool(&self) -> MemorySearch<C> {
        MemorySearch {
            db: self.db.clone(),
            scope: self.scope.clone(),
        }
    }

    async fn run(&self, tool_use: &ToolUseBlock) -> Result<ToolResultBlock> {
        let id = tool_use.id.clone();
        let command = match Command::deserialize(&tool_use.input) {
            Ok(command) => command,
            Err(error) => {
                return Ok(ToolResultBlock::error(
                    id,
                    format!("Invalid memory input: {error}"),
                ));
            }
        };
        match self.execute(command).await {
            Ok(reply) => Ok(ToolResultBlock::new(id).with_string_content(reply)),
            Err(Failure::Model(message)) => Ok(ToolResultBlock::error(id, message)),
            Err(Failure::Store(error)) => Err(error),
        }
    }

    async fn execute(&self, command: Command) -> std::result::Result<String, Failure> {
        match command {
            Command::View { path, view_range } => {
                check_path(&path)?;
                self.view(&path, view_range).await
            }
            Command::Create { path, file_text } => {
                check_file_path(&path)?;
                if self.is_directory(&path).await? {
                    return Err(model_error(format!("{path} is a directory")));
                }
                for (index, _) in path.match_indices('/').skip(1) {
                    let parent = &path[..index];
                    if self.file(parent).await?.is_some() {
                        return Err(model_error(format!("{parent} is a file")));
                    }
                }
                self.put(&path, &file_text).await?;
                Ok(format!("File created successfully at: {path}"))
            }
            Command::StrReplace {
                path,
                old_str,
                new_str,
            } => {
                check_file_path(&path)?;
                let text = self.require_file(&path).await?;
                if old_str.is_empty() {
                    return Err(model_error("old_str must not be empty"));
                }
                match text.matches(&old_str).count() {
                    0 => Err(model_error(format!(
                        "No replacement was performed, old_str did not appear verbatim in {path}"
                    ))),
                    1 => {
                        self.put(&path, &text.replacen(&old_str, &new_str, 1))
                            .await?;
                        Ok(format!("The file {path} has been edited."))
                    }
                    count => Err(model_error(format!(
                        "No replacement was performed. old_str appears {count} times in {path}; \
                         include more context so it matches exactly once."
                    ))),
                }
            }
            Command::Insert {
                path,
                insert_line,
                insert_text,
            } => {
                check_file_path(&path)?;
                let text = self.require_file(&path).await?;
                let edited = insert_lines(&text, insert_line, &insert_text)?;
                self.put(&path, &edited).await?;
                Ok(format!("The file {path} has been edited."))
            }
            Command::Delete { path } => {
                check_path(&path)?;
                if self.delete(&path).await? {
                    Ok(format!("Successfully deleted {path}"))
                } else {
                    Err(model_error(format!("{path}: no such file or directory")))
                }
            }
            Command::Rename { old_path, new_path } => {
                check_path(&old_path)?;
                check_path(&new_path)?;
                if old_path == ROOT || new_path == ROOT {
                    return Err(model_error(format!("{ROOT} cannot be renamed")));
                }
                self.rename(&old_path, &new_path).await?;
                Ok(format!("Successfully renamed {old_path} to {new_path}"))
            }
        }
    }

    async fn view(
        &self,
        path: &str,
        view_range: Option<[i64; 2]>,
    ) -> std::result::Result<String, Failure> {
        if let Some(text) = self.file(path).await? {
            return numbered(&text, view_range).map_err(Failure::Model);
        }
        let prefix = directory_prefix(path);
        let paths = self.paths_under(&prefix).await?;
        if paths.is_empty() && path != ROOT {
            return Err(model_error(format!("{path}: no such file or directory")));
        }
        let children: BTreeSet<String> = paths
            .iter()
            .map(|file| match file[prefix.len()..].split_once('/') {
                Some((directory, _)) => format!("{prefix}{directory}/"),
                None => file.clone(),
            })
            .collect();
        let mut listing = format!("Directory: {path}");
        for child in children {
            listing.push('\n');
            listing.push_str(&child);
        }
        Ok(listing)
    }

    async fn require_file(&self, path: &str) -> std::result::Result<String, Failure> {
        match self.file(path).await? {
            Some(text) => Ok(text),
            None => Err(model_error(format!("{path}: no such file or directory"))),
        }
    }

    async fn is_directory(&self, path: &str) -> Result<bool> {
        Ok(!self.paths_under(&directory_prefix(path)).await?.is_empty())
    }

    async fn file(&self, path: &str) -> Result<Option<String>> {
        let mut response = self
            .db
            .query("SELECT VALUE text FROM carpa_memory WHERE scope = $scope AND path = $path")
            .bind(("scope", self.scope.clone()))
            .bind(("path", path.to_string()))
            .await
            .map_err(|error| store_error(format!("reading {path}"), error))?;
        let texts: Vec<String> = response
            .take(0)
            .map_err(|error| store_error(format!("reading {path}"), error))?;
        Ok(texts.into_iter().next())
    }

    /// Every file path in the scope that starts with `prefix`, sorted.
    async fn paths_under(&self, prefix: &str) -> Result<Vec<String>> {
        let mut response = self
            .db
            .query(
                "SELECT VALUE path FROM carpa_memory \
                 WHERE scope = $scope AND string::starts_with(path, $prefix) ORDER BY path",
            )
            .bind(("scope", self.scope.clone()))
            .bind(("prefix", prefix.to_string()))
            .await
            .map_err(|error| store_error(format!("listing {prefix}"), error))?;
        response
            .take(0)
            .map_err(|error| store_error(format!("listing {prefix}"), error))
    }

    async fn put(&self, path: &str, text: &str) -> Result<()> {
        self.db
            .query(
                "UPSERT carpa_memory SET scope = $scope, path = $path, text = $text, \
                 updated_at = $at WHERE scope = $scope AND path = $path",
            )
            .bind(("scope", self.scope.clone()))
            .bind(("path", path.to_string()))
            .bind(("text", text.to_string()))
            .bind(("at", to_datetime(OffsetDateTime::now_utc())))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error(format!("writing {path}"), error))?;
        Ok(())
    }

    /// Deletes the file at `path`, or every file under it. Returns whether anything existed;
    /// the root always does.
    async fn delete(&self, path: &str) -> Result<bool> {
        let existed =
            path == ROOT || self.file(path).await?.is_some() || self.is_directory(path).await?;
        self.db
            .query(
                "DELETE carpa_memory WHERE scope = $scope \
                 AND (path = $path OR string::starts_with(path, $prefix))",
            )
            .bind(("scope", self.scope.clone()))
            .bind(("path", path.to_string()))
            .bind(("prefix", directory_prefix(path)))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error(format!("deleting {path}"), error))?;
        Ok(existed)
    }

    async fn rename(&self, old_path: &str, new_path: &str) -> std::result::Result<(), Failure> {
        let old_prefix = directory_prefix(old_path);
        let new_prefix = directory_prefix(new_path);
        let mut moves = Vec::new();
        if self.file(old_path).await?.is_some() {
            moves.push((old_path.to_string(), new_path.to_string()));
        } else {
            for file in self.paths_under(&old_prefix).await? {
                let moved = format!("{new_prefix}{}", &file[old_prefix.len()..]);
                moves.push((file, moved));
            }
        }
        if moves.is_empty() {
            return Err(model_error(format!(
                "{old_path}: no such file or directory"
            )));
        }
        if self.file(new_path).await?.is_some() || self.is_directory(new_path).await? {
            return Err(model_error(format!("{new_path} already exists")));
        }
        for (_, destination) in &moves {
            if self.file(destination).await?.is_some() {
                return Err(model_error(format!("{destination} already exists")));
            }
        }
        // One statement, so a crash cannot leave half of a directory moved.
        self.db
            .query(
                "UPDATE carpa_memory SET \
                 path = string::concat($new, string::slice(path, $old_len)), updated_at = $at \
                 WHERE scope = $scope \
                 AND (path = $old OR string::starts_with(path, $old_prefix))",
            )
            .bind(("scope", self.scope.clone()))
            .bind(("old", old_path.to_string()))
            .bind(("old_prefix", old_prefix))
            .bind(("new", new_path.to_string()))
            .bind(("old_len", old_path.chars().count() as i64))
            .bind(("at", to_datetime(OffsetDateTime::now_utc())))
            .await
            .and_then(|response| response.check())
            .map_err(|error| store_error(format!("renaming {old_path}"), error))?;
        Ok(())
    }
}

impl<C: Connection> ClientTool for Memory<C> {
    fn name(&self) -> &str {
        "memory"
    }

    fn definition(&self) -> ToolUnionParam {
        ToolUnionParam::Memory20250818(MemoryTool::new())
    }

    fn call<'a>(&'a self, tool_use: &'a ToolUseBlock) -> BoxFuture<'a, Result<ToolResultBlock>> {
        Box::pin(self.run(tool_use))
    }
}

impl<C: Connection> MemorySearch<C> {
    async fn run(&self, tool_use: &ToolUseBlock) -> Result<ToolResultBlock> {
        let id = tool_use.id.clone();
        let input = match SearchInput::deserialize(&tool_use.input) {
            Ok(input) => input,
            Err(error) => {
                return Ok(ToolResultBlock::error(
                    id,
                    format!(
                        "Invalid memory_search input: {error}. Send an object with \"query\" \
                         (a string) and optionally \"limit\" (1 to {MAX_TOOL_LIMIT})."
                    ),
                ));
            }
        };
        let limit = match input.limit {
            None => DEFAULT_TOOL_LIMIT,
            Some(limit @ 1..=MAX_TOOL_LIMIT) => limit as usize,
            Some(_) => {
                return Ok(ToolResultBlock::error(
                    id,
                    format!("limit must be between 1 and {MAX_TOOL_LIMIT}"),
                ));
            }
        };
        let hits = search(&self.db, &self.scope, &input.query, limit).await?;
        let text = if hits.is_empty() {
            "No memories match.".to_string()
        } else {
            hits.iter()
                .map(|hit| format!("{} ({:.2})\n{}", hit.path, hit.score, hit.text))
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        Ok(ToolResultBlock::new(id).with_string_content(text))
    }
}

impl<C: Connection> ClientTool for MemorySearch<C> {
    fn name(&self) -> &str {
        SEARCH_TOOL_NAME
    }

    fn definition(&self) -> ToolUnionParam {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Words to look for in your memory files."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TOOL_LIMIT,
                    "default": DEFAULT_TOOL_LIMIT,
                    "description": "How many files to return at most."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        });
        ToolUnionParam::CustomTool(
            ToolParam::new(SEARCH_TOOL_NAME.to_string(), schema).with_description(
                "Ranks your memory files by relevance to a query and returns the best matches \
                 with their text. Use the memory tool's view command to read one in full."
                    .to_string(),
            ),
        )
    }

    fn call<'a>(&'a self, tool_use: &'a ToolUseBlock) -> BoxFuture<'a, Result<ToolResultBlock>> {
        Box::pin(self.run(tool_use))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    query: String,
    limit: Option<u64>,
}

async fn search<C: Connection>(
    db: &Surreal<C>,
    scope: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryHit>> {
    let limit = limit.clamp(1, MAX_SEARCH_LIMIT) as i64;
    let mut response = db
        .query(
            "SELECT path, text, search::score(1) AS score FROM carpa_memory \
             WHERE scope = $scope AND text @1@ $query ORDER BY score DESC LIMIT $limit",
        )
        .bind(("scope", scope.to_string()))
        .bind(("query", query.to_string()))
        .bind(("limit", limit))
        .await
        .map_err(|error| store_error("searching memory", error))?;
    let rows: Vec<HitRow> = response
        .take(0)
        .map_err(|error| store_error("searching memory", error))?;
    Ok(rows
        .into_iter()
        .map(|row| MemoryHit {
            path: row.path,
            text: row.text,
            score: row.score,
        })
        .collect())
}

/// Checks that `path` is `/memories` or below it, with no empty or `..` segment.
fn check_path(path: &str) -> std::result::Result<(), Failure> {
    let valid = path == ROOT
        || path
            .strip_prefix("/memories/")
            .is_some_and(|rest| rest.split('/').all(|s| !s.is_empty() && s != ".."));
    if valid {
        Ok(())
    } else {
        Err(model_error(format!(
            "{path}: invalid path. Paths must be {ROOT} or start with {ROOT}/, with no empty or \
             .. segments."
        )))
    }
}

/// Like [`check_path`], and the path must not be the root directory.
fn check_file_path(path: &str) -> std::result::Result<(), Failure> {
    check_path(path)?;
    if path == ROOT {
        return Err(model_error(format!("{path} is a directory")));
    }
    Ok(())
}

/// `path` with one trailing slash, the prefix every file under it starts with.
fn directory_prefix(path: &str) -> String {
    format!("{path}/")
}

/// `text` with `cat -n` style line numbers, limited to `view_range` (1-based, inclusive, an end
/// of -1 meaning the last line).
fn numbered(text: &str, view_range: Option<[i64; 2]>) -> std::result::Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let count = lines.len() as i64;
    let (start, end) = match view_range {
        None => (1, count),
        Some([start, end]) => {
            let end = if end == -1 { count } else { end };
            if start < 1 || start > count || end < start || end > count {
                return Err(format!(
                    "Invalid view_range [{start}, {end}]: the file has {count} lines"
                ));
            }
            (start, end)
        }
    };
    Ok(lines[(start - 1) as usize..end as usize]
        .iter()
        .zip(start..)
        .map(|(line, number)| format!("{number:>6}\t{line}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// `text` with `insert_text` placed after line `insert_line` (0 puts it first).
fn insert_lines(
    text: &str,
    insert_line: i64,
    insert_text: &str,
) -> std::result::Result<String, Failure> {
    let mut lines: Vec<&str> = text.lines().collect();
    let count = lines.len();
    let after = usize::try_from(insert_line)
        .ok()
        .filter(|after| *after <= count)
        .ok_or_else(|| {
            model_error(format!(
                "Invalid insert_line {insert_line}: the file has {count} lines, so use 0 to \
                 {count}"
            ))
        })?;
    let inserted = insert_text.strip_suffix('\n').unwrap_or(insert_text);
    lines.splice(after..after, inserted.split('\n'));
    let mut edited = lines.join("\n");
    if text.ends_with('\n') {
        edited.push('\n');
    }
    Ok(edited)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use surrealdb::engine::local::Db;

    use super::*;
    use crate::surreal::test_db::temp_db;
    use crate::types::ToolResultBlockContent;

    async fn run(tool: &impl ClientTool, input: Value) -> ToolResultBlock {
        let tool_use = ToolUseBlock::new("toolu_1", tool.name(), input);
        tool.call(&tool_use).await.expect("tool call")
    }

    fn text_of(result: &ToolResultBlock) -> String {
        match &result.content {
            Some(ToolResultBlockContent::String(text)) => text.clone(),
            other => panic!("unexpected content: {other:?}"),
        }
    }

    async fn memory(scope: &str) -> Memory<Db> {
        Memory::try_new(temp_db().await, scope)
            .await
            .expect("memory")
    }

    async fn ok(memory: &Memory<Db>, input: Value) -> String {
        let result = run(memory, input).await;
        assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
        text_of(&result)
    }

    async fn fails(memory: &Memory<Db>, input: Value) -> String {
        let result = run(memory, input).await;
        assert_eq!(result.is_error, Some(true), "{}", text_of(&result));
        text_of(&result)
    }

    async fn create(memory: &Memory<Db>, path: &str, text: &str) {
        ok(
            memory,
            json!({"command": "create", "path": path, "file_text": text}),
        )
        .await;
    }

    #[tokio::test]
    async fn a_file_cannot_become_a_directory() {
        let memory = memory("a").await;
        create(&memory, "/memories/a.md", "one").await;
        fails(
            &memory,
            json!({"command": "create", "path": "/memories/a.md/b.md", "file_text": "two"}),
        )
        .await;
    }

    #[tokio::test]
    async fn renaming_a_directory_moves_every_file() {
        let memory = memory("a").await;
        create(&memory, "/memories/d/x.md", "x").await;
        create(&memory, "/memories/d/y/z.md", "z").await;
        ok(
            &memory,
            json!({"command": "rename", "old_path": "/memories/d", "new_path": "/memories/e"}),
        )
        .await;
        let view = |path: &str| json!({"command": "view", "path": path});
        assert!(ok(&memory, view("/memories/e/x.md")).await.contains('x'));
        assert!(ok(&memory, view("/memories/e/y/z.md")).await.contains('z'));
        fails(&memory, view("/memories/d")).await;
    }

    #[tokio::test]
    async fn create_then_view_numbers_lines() {
        let memory = memory("a").await;
        let reply = ok(
            &memory,
            json!({"command": "create", "path": "/memories/n.md", "file_text": "one\ntwo"}),
        )
        .await;
        assert_eq!(reply, "File created successfully at: /memories/n.md");
        let view = ok(
            &memory,
            json!({"command": "view", "path": "/memories/n.md"}),
        )
        .await;
        assert_eq!(view, "     1\tone\n     2\ttwo");
    }

    #[tokio::test]
    async fn view_root_lists_files_and_subdirectories() {
        let memory = memory("a").await;
        create(&memory, "/memories/b.md", "b").await;
        create(&memory, "/memories/dir/x.md", "x").await;
        create(&memory, "/memories/dir/deep/y.md", "y").await;
        let listing = ok(&memory, json!({"command": "view", "path": "/memories"})).await;
        assert_eq!(
            listing,
            "Directory: /memories\n/memories/b.md\n/memories/dir/"
        );
        let listing = ok(&memory, json!({"command": "view", "path": "/memories/dir"})).await;
        assert_eq!(
            listing,
            "Directory: /memories/dir\n/memories/dir/deep/\n/memories/dir/x.md"
        );
        fails(
            &memory,
            json!({"command": "view", "path": "/memories/nope"}),
        )
        .await;
    }

    #[tokio::test]
    async fn view_range_picks_lines() {
        let memory = memory("a").await;
        create(&memory, "/memories/n.md", "1\n2\n3\n4").await;
        let view = |range: Value| json!({"command": "view", "path": "/memories/n.md", "view_range": range});
        assert_eq!(
            ok(&memory, view(json!([2, 3]))).await,
            "     2\t2\n     3\t3"
        );
        assert_eq!(ok(&memory, view(json!([4, -1]))).await, "     4\t4");
        fails(&memory, view(json!([3, 9]))).await;
        fails(&memory, view(json!([0, 2]))).await;
    }

    #[tokio::test]
    async fn str_replace_needs_exactly_one_match() {
        let memory = memory("a").await;
        create(&memory, "/memories/n.md", "cat dog dog").await;
        let edit = |old: &str| {
            json!({"command": "str_replace", "path": "/memories/n.md", "old_str": old,
                   "new_str": "bird"})
        };
        let reply = ok(&memory, edit("cat")).await;
        assert_eq!(reply, "The file /memories/n.md has been edited.");
        let message = fails(&memory, edit("dog")).await;
        assert!(message.contains("2 times"), "{message}");
        let message = fails(&memory, edit("fish")).await;
        assert!(
            message.starts_with("No replacement was performed"),
            "{message}"
        );
        let view = ok(
            &memory,
            json!({"command": "view", "path": "/memories/n.md"}),
        )
        .await;
        assert_eq!(view, "     1\tbird dog dog");
    }

    #[tokio::test]
    async fn insert_at_start_and_after_a_line() {
        let memory = memory("a").await;
        create(&memory, "/memories/n.md", "a\nb").await;
        let insert = |line: i64, text: &str| {
            json!({"command": "insert", "path": "/memories/n.md", "insert_line": line,
                   "insert_text": text})
        };
        ok(&memory, insert(0, "first")).await;
        ok(&memory, insert(1, "second")).await;
        let view = ok(
            &memory,
            json!({"command": "view", "path": "/memories/n.md"}),
        )
        .await;
        assert_eq!(view, "     1\tfirst\n     2\tsecond\n     3\ta\n     4\tb");
        fails(&memory, insert(9, "x")).await;
    }

    #[tokio::test]
    async fn delete_removes_a_file_and_a_directory() {
        let memory = memory("a").await;
        create(&memory, "/memories/one.md", "1").await;
        create(&memory, "/memories/d/two.md", "2").await;
        create(&memory, "/memories/d/three.md", "3").await;
        let reply = ok(
            &memory,
            json!({"command": "delete", "path": "/memories/one.md"}),
        )
        .await;
        assert_eq!(reply, "Successfully deleted /memories/one.md");
        ok(&memory, json!({"command": "delete", "path": "/memories/d"})).await;
        let listing = ok(&memory, json!({"command": "view", "path": "/memories"})).await;
        assert_eq!(listing, "Directory: /memories");
        fails(&memory, json!({"command": "delete", "path": "/memories/d"})).await;
    }

    #[tokio::test]
    async fn rename_moves_files_and_refuses_to_overwrite() {
        let memory = memory("a").await;
        create(&memory, "/memories/old.md", "old").await;
        create(&memory, "/memories/other.md", "other").await;
        create(&memory, "/memories/d/x.md", "x").await;
        let rename =
            |from: &str, to: &str| json!({"command": "rename", "old_path": from, "new_path": to});
        let reply = ok(&memory, rename("/memories/old.md", "/memories/new.md")).await;
        assert_eq!(
            reply,
            "Successfully renamed /memories/old.md to /memories/new.md"
        );
        fails(&memory, rename("/memories/new.md", "/memories/other.md")).await;
        fails(&memory, rename("/memories/gone.md", "/memories/z.md")).await;
        ok(&memory, rename("/memories/d", "/memories/e")).await;
        let listing = ok(&memory, json!({"command": "view", "path": "/memories"})).await;
        assert_eq!(
            listing,
            "Directory: /memories\n/memories/e/\n/memories/new.md\n/memories/other.md"
        );
    }

    #[tokio::test]
    async fn bad_paths_and_commands_are_error_results() {
        let memory = memory("a").await;
        fails(&memory, json!({"command": "view", "path": "/etc/passwd"})).await;
        fails(
            &memory,
            json!({"command": "view", "path": "/memories/../x"}),
        )
        .await;
        fails(&memory, json!({"command": "view", "path": "/memoriesx"})).await;
        fails(&memory, json!({"command": "view", "path": "/memories//x"})).await;
        fails(&memory, json!({"command": "explode", "path": "/memories"})).await;
        fails(&memory, json!({"path": "/memories"})).await;
        fails(&memory, json!({"command": "create", "path": "/memories"})).await;
    }

    #[tokio::test]
    async fn scopes_do_not_see_each_other() {
        let db = temp_db().await;
        let alice = Memory::try_new(db.clone(), "alice").await.expect("alice");
        let bob = Memory::try_new(db, "bob").await.expect("bob");
        create(&alice, "/memories/secret.md", "alice owns this").await;
        let listing = ok(&bob, json!({"command": "view", "path": "/memories"})).await;
        assert_eq!(listing, "Directory: /memories");
        fails(
            &bob,
            json!({"command": "view", "path": "/memories/secret.md"}),
        )
        .await;
        assert!(bob.search("alice", 5).await.expect("search").is_empty());
        assert_eq!(alice.search("alice", 5).await.expect("search").len(), 1);
    }

    #[tokio::test]
    async fn search_ranks_the_matching_file_first() {
        let memory = memory("a").await;
        create(
            &memory,
            "/memories/pets.md",
            "The user keeps two cats and a dog",
        )
        .await;
        create(
            &memory,
            "/memories/work.md",
            "The user works on pump controllers",
        )
        .await;
        create(&memory, "/memories/food.md", "The user likes pizza").await;
        let hits = memory.search("pump controller", 10).await.expect("search");
        assert_eq!(hits[0].path, "/memories/work.md");
        assert!(hits[0].text.contains("pump"));
        assert!(hits.iter().all(|hit| hit.path != "/memories/food.md"));
    }

    #[tokio::test]
    async fn memory_search_tool_formats_hits_and_empty_scopes() {
        let memory = memory("a").await;
        let tool = memory.search_tool();
        assert_eq!(tool.name(), "memory_search");
        let empty = run(&tool, json!({"query": "anything"})).await;
        assert_eq!(text_of(&empty), "No memories match.");
        create(&memory, "/memories/work.md", "pump controllers").await;
        let found = run(&tool, json!({"query": "pump"})).await;
        let text = text_of(&found);
        assert!(text.starts_with("/memories/work.md ("), "{text}");
        assert!(text.ends_with(")\npump controllers"), "{text}");
        let bad = run(&tool, json!({"limit": 3})).await;
        assert_eq!(bad.is_error, Some(true));
    }
}
