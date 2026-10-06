use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

/// The future `Tool::execute` returns.
///
/// `execute` is async, but the trait is used as `Box<dyn Tool>` in the
/// registry, and native `async fn` in traits is not dyn-compatible — so
/// the signature is the manual desugaring: a boxed, `Send` future. The
/// `tool!` macro hides the boxing, so tool bodies read like plain async
/// functions.
pub type ToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, Box<dyn std::error::Error>>> + Send + 'a>>;

/// A tool the model can call. `Send + Sync` because the boxed futures
/// borrow `&self` and must be usable from any worker thread.
pub trait Tool: Send + Sync {
    fn name(&self) -> String;
    fn desc(&self) -> String;
    fn params(&self) -> Option<Value>;
    fn execute<'a>(&'a self, parameters: Value) -> ToolFuture<'a>;
}