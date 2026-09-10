macro_rules! define_tool {
    (
        $tool:ident, $params:ident, $desc:literal,
        execute($args:ident) { $($body:tt)* }
    ) => {
        pub struct $tool;
        impl crate::sven::tool::SvenTool for $tool {
            fn name(&self) -> String { String::from(stringify!($tool)) }
            fn desc(&self) -> String { String::from($desc) }
            fn params(&self) -> Option<Value> {
                let mut schema = schemars::schema_for!($params);
                schema.remove("$schema");
                Some(json!(schema))
            }
            fn execute(&self, params: Value) -> Result<String, Box<dyn std::error::Error>> {
                let $args: $params = serde_json::from_value(params)?;
                $($body)*
            }
        }
    };
}
pub(crate) use define_tool;
