use std::process::Command;

use serde_json::{Value, json};

use crate::sven::security;
use crate::sven::tool::Tool;
use crate::sven::tool_error::ToolError;

pub struct ListFiles;
impl Tool for ListFiles {
    fn name(&self) -> String {
        "list_files".to_string()
    }

    fn description(&self) -> String {
        "List files in current directory".to_string()
    }

    fn parameters(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "file path"
                }
            }
        }))
    }

    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
        let mut command = Command::new("ls");
        command.arg("-1");
        if let Some(path) = parameters.get("path").and_then(|p| p.as_str()) {
            let _ = security::is_inside_cwd(&path)?;
            command.arg(path);
        }
        let output = command.output()?;

        let out = String::from_utf8(output.stdout)?;
        Ok(out.trim().to_string())
    }
}
