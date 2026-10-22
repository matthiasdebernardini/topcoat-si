//! Checks a pack's examples against a real sandbox.

use std::sync::Arc;

use super::{CodeMode, Limits, Pack, Pool};
use crate::error::{Error, Result};

/// Runs every [`Example`](super::Example) of the pack `pack` builds, each in a fresh
/// `CodeMode` built from new `fakes` plus a new pack, and fails naming the first example whose
/// tool result text differs from its `expect`.
///
/// `fakes` builds packs of host functions returning canned JSON in place of the real ones, such
/// as a fake `firecrawl` pack for `research`. Both are called once per example, so no host-call
/// budget or fake state carries from one example to the next.
pub async fn check_pack(
    pool: Arc<Pool>,
    pack: impl Fn() -> Pack,
    fakes: impl Fn() -> Vec<Pack>,
) -> Result<()> {
    let checked = pack();
    let name = checked.name;
    for (function, example) in checked.examples {
        let mut packs = fakes();
        packs.push(pack());
        let code = CodeMode::try_new(pool.clone(), packs, Limits::default())?;
        let reply = code.execute(example.code).await?;
        code.finish().await?;
        if reply.text != example.expect {
            return Err(Error::validation(
                format!(
                    "pack {name}: the {function} example failed\ncode: {}\nexpected: {}\ngot: {}",
                    example.code, example.expect, reply.text
                ),
                Some(function),
            ));
        }
    }
    Ok(())
}
