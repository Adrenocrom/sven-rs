use std::process::Command;

use serde_json::{Value, json};

use crate::sven::tool::Tool;
use crate::sven::tool_error::ToolError;

pub struct WebSearch;

impl WebSearch {}

impl Tool for WebSearch {
    fn name(&self) -> String {
        "web_search".to_string()
    }

    fn description(&self) -> String {
        r#"Search the web via DuckDuckGo.
    Notes
    -----
    The results are intentionally minimal – only the raw URLs are returned.  
    If you want to investigate any of the links further, use the `webfetch`
    helper (or your own HTTP client) on the *interesting* URLs to fetch and
    inspect their content."#
            .to_string()
    }

    fn parameters(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": r#"The search string to submit to DuckDuckGo. This can be a simple keyword, a phrase, or a more elaborate Boolean expression (e.g., "site:example.com \"error code\"")."#
                }
            }
        }))
    }

    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
        let query = match parameters.get("query").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => {
                return Err(ToolError::InvalidParameters(
                    "'query' is a required parameter.".to_string(),
                ));
            }
        };

        let mut command = Command::new("ddgr");
        command.arg("--noprompt");
        command.arg(query);
        let output = command.output()?;
        let out = String::from_utf8(output.stdout)?;
        Ok(out)
    }
}
