use std::process::Command;

use serde_json::{Value, json};

use crate::sven::tool::Tool;
use crate::sven::tool_error::ToolError;

pub struct FindTool;
impl Tool for FindTool {
    fn name(&self) -> String {
        "find".to_string()
    }

    fn description(&self) -> String {
        "Search for files whose names match *pattern*.".to_string()
    }

    fn parameters(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": r#"Unix shell‑style wildcard pattern (e.g. "*.py")"#
                }
            }
        }))
    }

    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
        let pattern = match parameters.get("pattern").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => {
                return Err(ToolError::InvalidParameters(
                    "'pattern' is a required string.".to_string(),
                ));
            }
        };

        let mut command = Command::new("find");
        command.arg(".");
        command.arg("-name");
        command.arg(pattern);
        let output = command.output()?;
        let out = String::from_utf8(output.stdout)?;
        Ok(out.trim().to_string())
    }
}
