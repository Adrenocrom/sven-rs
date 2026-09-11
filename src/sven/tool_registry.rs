use std::collections::HashMap;

use serde_json::{Value, json};

use crate::sven::tool::Tool;

pub struct ToolRegistry {
    pub tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name(), tool);
    }

    pub fn get_tool(&self, name: &str) -> Option<&Box<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn generate_tool_definitions(&self) -> Value {
        let tools_definitions: Vec<_> = self
            .tools
            .iter()
            .map(|entry| {
                let tool = entry.1;
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.desc(),
                        "parameters": tool.params()
                    }
                })
            })
            .collect();
        json!(tools_definitions)
    }
}
