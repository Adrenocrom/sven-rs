use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::sven::tool::Tool;

/// Registry of the tools the model can call.
///
/// A `BTreeMap` keeps the definitions in a stable alphabetical order (a
/// `HashMap` would reshuffle the tool list on every request), and the
/// definitions are built in `register` so the schemars schemas are not
/// regenerated on every chat round.
pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn Tool>>,
    definitions: Vec<Value>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            definitions: Vec::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name(), tool);
        self.rebuild_definitions();
    }

    fn rebuild_definitions(&mut self) {
        self.definitions = self
            .tools
            .values()
            .map(|tool| {
                let params = tool
                    .params()
                    .unwrap_or_else(|| json!({"properties": {}, "type": "object"}));
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.desc(),
                        "parameters": params
                    }
                })
            })
            .collect();
    }

    pub fn get_tool(&self, name: &str) -> Option<&Box<dyn Tool>> {
        self.tools.get(name)
    }

    /// Tool definitions sent with each chat request.
    pub fn tool_definitions(&self) -> &[Value] {
        &self.definitions
    }
}