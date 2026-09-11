use serde_json::Value;

use crate::sven::tool_error::ToolError;

pub trait Tool {
    fn name(&self) -> String;
    fn desc(&self) -> String;
    fn params(&self) -> Option<Value>;
    fn execute(&self, parameters: Value) -> Result<String, Box<dyn std::error::Error>>;
}
