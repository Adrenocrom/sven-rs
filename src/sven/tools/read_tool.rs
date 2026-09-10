use std::io::BufRead;

use serde_json::{Value, json};

use crate::sven::security;
use crate::sven::tool::Tool;
use crate::sven::tool_error::ToolError;

pub struct ReadTool;
impl Tool for ReadTool {
    fn name(&self) -> String {
        "read".to_string()
    }

    fn description(&self) -> String {
        "Read a file from a given path, with optional line offset and count.".to_string()
    }

    fn parameters(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "file path"
                },
                "num_lines": {
                    "type": "integer",
                    "description": "number of lines to read (default: whole file)"
                },
                "offset": {
                    "type": "integer",
                    "description": "skip this many leading lines before reading"
                }
            }
        }))
    }

    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
        let path = match parameters.get("path").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => {
                return Err(ToolError::InvalidParameters(
                    "'path' is a required string.".to_string(),
                ));
            }
        };

        let _ = security::is_inside_cwd(&path)?;

        let offset = match parameters.get("offset").and_then(|o| o.as_i64()) {
            Some(n) if n > 0 => n as usize,
            _ => 0,
        };

        let num_lines = match parameters.get("num_lines").and_then(|n| n.as_i64()) {
            Some(n) if n > 0 => Some((n as usize) + offset),
            _ => None,
        };

        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(&file);
        let mut result = String::new();

        for (idx, line) in reader.lines().enumerate() {
            if idx < offset {
                continue;
            }
            if let Some(num_lines) = num_lines {
                if idx >= num_lines {
                    break;
                }
            }

            let line = line?;
            result.push_str(&line);
            result.push('\n');
        }

        Ok(result)
    }
}
