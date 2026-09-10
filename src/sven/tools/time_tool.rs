use chrono::Local;
use serde_json::Value;

use crate::sven::{tool::Tool, tool_error::ToolError};

pub struct TimeTool;
impl Tool for TimeTool {
    fn name(&self) -> String {
        "time_tool".to_string()
    }

    fn description(&self) -> String {
        "Gets  current time".to_string()
    }

    fn parameters(&self) -> Option<Value> {
        None
    }

    fn execute_tool(&self, _parameters: Value) -> Result<String, ToolError> {
        let now = Local::now();
        Ok(now.to_string())
    }
}
