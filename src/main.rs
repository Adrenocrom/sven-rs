use std::{collections::HashMap, error::Error, slice::SliceIndex};

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Value, json};

trait Tool {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn params(&self) -> Option<Value>;
    fn execute(&self, params: Value) -> Result<String, Box<dyn Error>>;
}


macro_rules! define_tool {
    (
        $tool:ident, $params:ident, $name:literal, $desc:literal,
        params { $($field:ident : $ty:ty),* $(,)? },
        execute($args:ident) { $($body:tt)* }
    ) => {
        #[derive(Debug, Deserialize, JsonSchema)]
        struct $params {
            $($field: $ty,)*
        }

        struct $tool;

        impl Tool for $tool {
            fn name(&self) -> String { String::from($name) }
            fn description(&self) -> String { String::from($desc) }
            fn params(&self) -> Option<Value> {
                Some(json!(schema_for!($params)))
            }
            fn execute(&self, params: Value) -> Result<String, Box<dyn Error>> {
                let $args: $params = serde_json::from_value(params)?;
                $($body)*
            }
        }
    };
}

macro_rules! tools {
    ($($t:ident),* $(,)?) => {{
        let mut map: HashMap<String, Box<dyn Tool>> = HashMap::new();
        $(
            let t: Box<dyn Tool> = Box::new($t);
            map.insert(t.name(), t);
        )*
        map
    }};
}

define_tool!(WriteTool, WriteToolParams, "write_tool", "writes content to a file",
    params { path: String },
    execute(args) { 
        let result = format!("written to: {}",  args.path).to_string();
        Ok(result) 
    }
);

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadToolParams {
    path: String,
    offset: Option<u32>,
    take: Option<u32>
}

struct ReadTool;
impl Tool for ReadTool {
    fn name(&self) -> String { String::from("read_tool") }
    fn description(&self) -> String { String::from("read text from a file") }
    fn params(&self) -> Option<Value> {
        Some(json!(schema_for!(ReadToolParams)))
    }

    fn execute(&self, params: Value) -> Result<String, Box<dyn Error>> {
        let args : ReadToolParams =  serde_json::from_value(params)?;
        Ok(args.path)
    }

}

#[derive(Debug, Deserialize, JsonSchema)]
struct ManToolParams {
    name: String,
}

struct ManTool;
impl Tool for ManTool {
    fn name(&self) -> String { String::from("man_tool") }
    fn description(&self) -> String { String::from("calls man") }
    fn params(&self) -> Option<Value> {
        Some(json!(schema_for!(ManToolParams)))
    }

    fn execute(&self, params: Value) -> Result<String, Box<dyn Error>> {
        let args : ManToolParams =  serde_json::from_value(params)?;
        Ok(args.name)
    }

}

fn main() {
    let tools = tools![ReadTool, WriteTool, ManTool];
    
    let r_t = match tools.get("write_tool") {
        Some(t) => t,
        None => return
    };

    let result = match r_t.execute(json!({"path":"example.txt"})) {
        Ok(res) => res,
        Err(e) => e.to_string()
    };

    println!("{}", result);
}
