//! Rust functions the sandbox can call.

use std::future::Future;
use std::marker::PhantomData;

use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::HostError;

/// A function the sandbox can call. Must be read-only: the crate cannot enforce this.
pub trait HostFunction: Send + Sync {
    /// Python name, unique across all packs in one CodeMode.
    fn name(&self) -> &str;
    /// The Python signature after `async def`, keyword-only, with a return annotation:
    /// `search(*, query: str, limit: int = 5, scrape: bool = False) -> list[dict]`.
    fn signature(&self) -> &str;
    /// One line, shown in the tool description.
    fn summary(&self) -> &str;
    /// Runs one call. `args` is a JSON object of the keyword arguments as passed.
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<Value, HostError>>;
}

/// A [`HostFunction`] whose keyword arguments deserialize into `A`.
///
/// `A` is a serde struct with `#[serde(deny_unknown_fields)]` and defaults for optional fields.
/// Arguments that don't deserialize raise `TypeError` naming the function, without running
/// `function` or charging the host-call budget.
#[must_use]
pub fn host_fn<A, F, Fut>(
    name: impl Into<String>,
    signature: impl Into<String>,
    summary: impl Into<String>,
    function: F,
) -> impl HostFunction
where
    A: DeserializeOwned + Send + 'static,
    F: Fn(A) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, HostError>> + Send + 'static,
{
    TypedHostFn {
        name: name.into(),
        signature: signature.into(),
        summary: summary.into(),
        function,
        args: PhantomData,
    }
}

struct TypedHostFn<A, F> {
    name: String,
    signature: String,
    summary: String,
    function: F,
    args: PhantomData<fn(A)>,
}

impl<A, F, Fut> HostFunction for TypedHostFn<A, F>
where
    A: DeserializeOwned + Send + 'static,
    F: Fn(A) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, HostError>> + Send + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn signature(&self) -> &str {
        &self.signature
    }

    fn summary(&self) -> &str {
        &self.summary
    }

    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<Value, HostError>> {
        match serde_json::from_value::<A>(args) {
            Ok(args) => Box::pin((self.function)(args)),
            Err(error) => {
                let message = format!("{}(): {error}", self.name);
                Box::pin(async move { Err(HostError::BadArgs(message)) })
            }
        }
    }
}
