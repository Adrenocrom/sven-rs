// `execute` returns a boxed future (see `tool.rs`): native `async fn`
// in traits is not dyn-compatible, so the macro performs the same
// desugaring the `async-trait` crate would — without the dependency.
//
// The body keeps its original type, `Result<String, …>`: its `?`s and
// `return Err`s leave the future unchanged, and the macro wraps the
// String into the `ToolOutput` the trait returns. A tool that needs to
// return attachments (images, audio) implements `Tool` by hand instead
// — see `McpTool`.
macro_rules! tool {
    (
        $tool:ident, $params:ident, $desc:literal,
        execute($args:ident) { $($body:tt)* }
    ) => {
        pub struct $tool;
        impl crate::sven::tool::Tool for $tool {
            fn name(&self) -> String { String::from(stringify!($tool)) }
            fn desc(&self) -> String { String::from($desc) }
            fn params(&self) -> Option<serde_json::Value> {
                let mut schema = schemars::schema_for!($params);
                schema.remove("$schema");
                Some(serde_json::json!(schema))
            }
            fn execute<'a>(&'a self, params: serde_json::Value) -> crate::sven::tool::ToolFuture<'a> {
                Box::pin(async move {
                    let $args: $params = serde_json::from_value(params)?;
                    let result: Result<String, Box<dyn std::error::Error>> = { $($body)* };
                    Ok(result?.into())
                })
            }
        }
    };

    (
        $tool:ident, $desc:literal,
        execute() { $($body:tt)* }
    ) => {
        pub struct $tool;
        impl crate::sven::tool::Tool for $tool {
            fn name(&self) -> String { String::from(stringify!($tool)) }
            fn desc(&self) -> String { String::from($desc) }
            fn params(&self) -> Option<serde_json::Value> {
                None
            }
            fn execute<'a>(&'a self, _params: serde_json::Value) -> crate::sven::tool::ToolFuture<'a> {
                Box::pin(async move {
                    let result: Result<String, Box<dyn std::error::Error>> = { $($body)* };
                    Ok(result?.into())
                })
            }
        }
    };
}

pub(crate) use tool;