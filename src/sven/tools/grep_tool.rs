use std::process::Command;

use serde_json::{Value, json};

use crate::sven::tool::Tool;
use crate::sven::tool_error::ToolError;

pub struct GrepTool;

impl GrepTool {}

impl Tool for GrepTool {
    fn name(&self) -> String {
        "grep".to_string()
    }

    fn description(&self) -> String {
        "Search for a regex pattern in given files or stdin.".to_string()
    }

    fn parameters(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for."
                }
            }
        }))
    }

    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
        let pattern = match parameters.get("pattern").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => return Err(ToolError::MissingParameter("pattern".to_string())),
        };

        let mut command = Command::new("grep");
        command.arg("-rni");
        command.arg(pattern);
        let output = command.output()?;
        let out = String::from_utf8(output.stdout)?;
        Ok(out.trim().to_string())
    }
}
