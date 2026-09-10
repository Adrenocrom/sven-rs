use std::process::Command;

use serde_json::{Value, json};

use crate::sven::tool::Tool;
use crate::sven::tool_error::ToolError;

pub struct ManPageTool;

impl ManPageTool {}

impl Tool for ManPageTool {
    fn name(&self) -> String {
        "manpage".to_string()
    }

    fn description(&self) -> String {
        "Displays the first page of a manual page. Usage: manpage <name>".to_string()
    }

    fn parameters(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "name of the man page to display"
                }
            }
        }))
    }

    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
        let name = match parameters.get("name").and_then(|p| p.as_str()) {
            Some(name) => name,
            None => return Err(ToolError::MissingParameter("name".to_string())),
        };

        let mut command = Command::new("man");
        command.arg(name);
        let output = command.output()?;
        let out = String::from_utf8(output.stdout)?;
        Ok(out.to_string())
    }
}
