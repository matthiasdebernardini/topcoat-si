//! The shared worker pool.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use monty_pool::{Pool, PoolConfig};

use super::limits::REQUEST_TIMEOUT;
use crate::error::{Error, ErrorKind, Result};

const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(10);

/// Creates the shared worker pool. `monty` is the path to the monty-runtime 1.2 binary.
///
/// One pool serves every conversation in the app, so `max_workers` caps the live sandboxes. A
/// conversation that finds every worker busy waits up to 10 seconds, then gets a "no sandbox
/// free" tool result. The pool kills a worker that leaves any one request unanswered for 120
/// seconds; the sandbox's own time limits stop a snippet well before that.
pub async fn pool(monty: impl Into<PathBuf>, max_workers: usize) -> Result<Arc<Pool>> {
    if max_workers == 0 {
        return Err(Error::validation(
            "max_workers must be at least 1",
            Some("max_workers".to_string()),
        ));
    }
    let mut config = PoolConfig::subprocess(monty);
    config.min_processes = 1;
    config.max_processes = max_workers;
    config.checkout_timeout = Some(CHECKOUT_TIMEOUT);
    config.request_timeout = Some(REQUEST_TIMEOUT);
    Pool::new(config).await.map(Arc::new).map_err(|error| {
        Error::new(ErrorKind::Sandbox, "could not start the sandbox pool").with_source(error)
    })
}
