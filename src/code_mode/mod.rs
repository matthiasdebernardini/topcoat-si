//! Code mode: Claude writes Python, a Monty sandbox runs it, and the host functions it calls are
//! answered by Rust.
//!
//! [`CodeMode`] is one client tool, `python`, for
//! [`Anthropic::run_tools`](crate::Anthropic::run_tools). The functions a script can call come
//! in [`Pack`]s: the built-in ones in [`packs`] and any the app adds with [`host_fn`] and Python
//! source. One snippet can search, scrape, classify, filter and report without the intermediate
//! data entering Claude's context.
//!
//! Workers are `monty` processes from `monty-runtime` 1.2, installed with
//! `cargo install monty-runtime --version 1.2.0 --locked`. Create one [`pool`] per app and one
//! [`CodeMode`] per conversation.
//!
//! ```no_run
//! # async fn run(monty_path: std::path::PathBuf) -> carpa::Result<()> {
//! use carpa::code_mode::{self, CodeMode, Limits, packs};
//!
//! // once per app
//! let pool = code_mode::pool(monty_path, 4).await?;
//! // once per conversation
//! let code = CodeMode::try_new(pool.clone(), vec![packs::reports()], Limits::default())?;
//! // ... pass `&code` to run_tools ...
//! code.finish().await?;
//! # Ok(())
//! # }
//! ```

mod host_error;
mod host_function;
mod limits;
mod pack;
pub mod packs;
mod pool;
pub mod testing;
mod tool;
mod value;

#[cfg(test)]
mod tests;

pub use host_error::{HostError, PyError};
pub use host_function::{HostFunction, host_fn};
pub use limits::Limits;
pub use monty_pool::Pool;
pub use pack::{Example, Pack};
pub use pool::pool;
pub use tool::CodeMode;
