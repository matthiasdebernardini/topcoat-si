//! Embedded SurrealDB behind the `surreal` feature: persistent memory for Claude, transcripts
//! and batch ids that survive a restart, a Firecrawl page cache, and a usage ledger. The
//! read-only code-mode pack is [`code_mode::packs::surreal`](crate::code_mode::packs::surreal).
//!
//! Everything here takes a [`Surreal`] handle and runs its own `DEFINE ... IF NOT EXISTS`
//! statements when built, so an app opens one database with [`open`] and hands it to each piece.
//! The engine is SurrealKV, pure Rust, one process per directory: it holds an exclusive lock, so
//! two binaries need two paths. The crate is re-exported as [`surrealdb`] for an
//! app's own tables; derive its records with
//! `#[derive(SurrealValue)] #[surreal(crate = "carpa::surrealdb::types")]`.

mod memory;
mod page_cache;
mod reader;
pub(crate) mod time;
mod transcripts;
mod usage_ledger;

pub use memory::{Memory, MemoryHit, MemorySearch};
pub use page_cache::{CachedPage, PageCache};
pub use reader::Reader;
pub use transcripts::{Batches, TrackedBatch, Transcript, Transcripts};
pub use usage_ledger::{ModelTotal, UsageLedger};

use std::path::Path;

use surrealdb::Surreal;
use surrealdb::engine::local::{Db, SurrealKv};

use crate::error::{Error, Result};

/// Opens, or creates, the SurrealKV database in the directory `path` and selects `namespace` and
/// `database`.
///
/// # Errors
///
/// [`ErrorKind::Store`](crate::ErrorKind::Store) when the directory cannot be opened, for example
/// because another process holds its lock.
pub async fn open(path: impl AsRef<Path>, namespace: &str, database: &str) -> Result<Surreal<Db>> {
    let path = path.as_ref();
    let db = Surreal::new::<SurrealKv>(path)
        .await
        .map_err(|error| store_error(format!("opening {}", path.display()), error))?;
    db.use_ns(namespace)
        .use_db(database)
        .await
        .map_err(|error| store_error(format!("selecting {namespace}/{database}"), error))?;
    Ok(db)
}

/// Wraps a SurrealDB error as [`ErrorKind::Store`](crate::ErrorKind::Store), keeping it as the
/// source.
pub(crate) fn store_error(context: impl Into<String>, error: surrealdb::Error) -> Error {
    Error::store(context, Some(Box::new(error)))
}

#[cfg(test)]
pub(crate) mod test_db {
    use std::sync::atomic::{AtomicU64, Ordering};

    use surrealdb::Surreal;
    use surrealdb::engine::local::Db;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A fresh SurrealKV database in a temp directory of its own. SurrealKV locks its directory,
    /// so every test gets a new one; the directory is left for the OS to clean.
    pub(crate) async fn temp_db() -> Surreal<Db> {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("carpa-surreal-{}-{n}", std::process::id()));
        super::open(&dir, "test", "test")
            .await
            .expect("open temp db")
    }
}
