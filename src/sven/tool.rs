use serde_json::Value;

use crate::sven::tool_error::ToolError;

pub trait Tool {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn parameters(&self) -> Option<Value>;
    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError>;
}

pub trait SvenTool {
    fn name(&self) -> String;
    fn desc(&self) -> String;
    fn params(&self) -> Option<Value>;
    fn execute(&self, parameters: Value) -> Result<String, Box<dyn std::error::Error>>;
}
