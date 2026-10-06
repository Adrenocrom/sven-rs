// `execute` returns a boxed future (see `tool.rs`): native `async fn`
// in traits is not dyn-compatible, so the macro performs the same
// desugaring the `async-trait` crate would — without the dependency.
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
                    $($body)*
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
                Box::pin(async move { $($body)* })
            }
        }
    };
}

pub(crate) use tool;