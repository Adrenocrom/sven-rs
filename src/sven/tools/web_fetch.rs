use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::sven::tool::Tool;
use crate::sven::tool_error::ToolError;

pub struct WebFetch;

impl WebFetch {}

impl Tool for WebFetch {
    fn name(&self) -> String {
        "web_fetch".to_string()
    }

    fn description(&self) -> String {
        "Does a GET request to a specified URL.".to_string()
    }

    fn parameters(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL starting with http:// or https://"
                }
            }
        }))
    }

    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
        let url = match parameters.get("url").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => {
                return Err(ToolError::InvalidParameters(
                    "'url' is a required parameter.".to_string(),
                ));
            }
        };

        let curl_output = match Command::new("curl")
            .arg("--silent")
            .arg("-L")
            .arg("-f")
            .arg("--")
            .arg(url)
            .stdout(Stdio::piped())
            .spawn()?
            .stdout
        {
            Some(stdout) => stdout,
            None => return Ok("".to_string()),
        };

        let pandoc_output = Command::new("pandoc")
            .arg("-f")
            .arg("html")
            .arg("-t")
            .arg("gfm")
            .stdin(Stdio::from(curl_output))
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?;

        let result = String::from_utf8(pandoc_output.stdout)?;
        Ok(result)
    }
}
