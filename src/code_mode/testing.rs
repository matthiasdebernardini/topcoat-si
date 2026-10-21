//! Checks a pack's examples against a real sandbox.

use std::sync::Arc;

use super::{CodeMode, Limits, Pack, Pool};
use crate::error::{Error, Result};

/// Runs every [`Example`](super::Example) of `pack`, each in a fresh session of a `CodeMode`
/// built from `fakes` plus `pack`, and fails naming the first example whose tool result text
/// differs from its `expect`.
///
/// `fakes` are packs of host functions returning canned JSON in place of the real ones, such as
/// a fake `firecrawl` pack for `research`.
pub async fn check_pack(pool: Arc<Pool>, pack: Pack, fakes: Vec<Pack>) -> Result<()> {
    let name = pack.name;
    let examples = pack.examples.clone();
    let mut packs = fakes;
    packs.push(pack);
    let code = CodeMode::new(pool, packs, Limits::default())?;
    for (function, example) in examples {
        let reply = code.execute(example.code).await?;
        code.end_session().await?;
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
    code.finish().await
}
