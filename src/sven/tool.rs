use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

use crate::sven::attachment::Attachment;

/// The future `Tool::execute` returns.
///
/// `execute` is async, but the trait is used as `Box<dyn Tool>` in the
/// registry, and native `async fn` in traits is not dyn-compatible — so
/// the signature is the manual desugaring: a boxed, `Send` future. The
/// `tool!` macro hides the boxing, so tool bodies read like plain async
/// functions.
pub type ToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolOutput, Box<dyn std::error::Error>>> + Send + 'a>>;

/// What a tool returns: text for the model, plus optional attachments
/// (images, audio, video) the backend may carry alongside it. Text-only
/// tools simply return `Ok(text.into())`; the agent's truncation applies
/// to the text only, never to the payloads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

impl ToolOutput {
    /// A text-only result.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
        }
    }
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

/// A tool the model can call. `Send + Sync` because the boxed futures
/// borrow `&self` and must be usable from any worker thread.
pub trait Tool: Send + Sync {
    fn name(&self) -> String;
    fn desc(&self) -> String;
    fn params(&self) -> Option<Value>;
    fn execute<'a>(&'a self, parameters: Value) -> ToolFuture<'a>;
}