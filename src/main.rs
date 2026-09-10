use std::{collections::HashMap, error::Error, slice::SliceIndex, string::FromUtf8Error};

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;

trait Tool {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn params(&self) -> Option<Value>;
    fn execute(&self, params: Value) -> Result<String, Box<dyn Error>>;
}

pub enum ToolError {
    MissingParameter,
}

impl From<FromUtf8Error> for ToolError {
    fn from(value: FromUtf8Error) -> Self {
        ToolError::MissingParameter
    }
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

macro_rules! define_tool2 {
    (
        $tool:ident, $params:ident, $desc:literal,
        execute($args:ident) { $($body:tt)* }
    ) => {
        struct $tool;

        impl Tool for $tool {
            fn name(&self) -> String { String::from(stringify!($tool)) }
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
    params { 
        path: String,
        content: String
    },
    execute(args) { 
        std::fs::write(&args.path, &args.content)?;   // ? converts io::Error → Box<dyn Error>
        Ok(format!("written to: {}", args.path))
    }
);

define_tool!(ReadTool, ReadToolParams, "read_tool", "reads content from file", 
    params {
        path: String,
        offset: Option<u32>,
        limit: Option<u32>
    },
    execute(args) { 
        if let  Some(offset) = args.offset {
            println!("offset; {}", offset);
        }
        if let  Some(limit) = args.limit {
            println!("limit; {}", limit);
        }
        Ok(format!("reading content from: {}",  args.path)) 
    }
);

define_tool!(ManTool, ManToolParams, "man_tool", "display man pages", 
    params {
        man_page: String,
    },
    execute(args) { 
        Ok(format!("displaying man page: {}",  args.man_page)) 
    }
);

#[derive(Deserialize, JsonSchema)]
struct WeatherToolParams {
    ///  city name for the weather prediction
    city: String
}
define_tool2!(WeatherTool, WeatherToolParams, "show current weather conditions", execute(args) {
    Ok(format!("current weather in {}: {}", args.city, "sunny".to_string())) 
});

fn main() {
    let tools = tools![WeatherTool, ReadTool, WriteTool, ManTool];
    println!("{}", stringify!(WeatherTool));
    
    let r_t = match tools.get("WeatherTool") {
        Some(t) => t,
        None => return
    };

    let p = r_t.params().expect("some");
    println!("params: {}", p.to_string());

    let result = match r_t.execute(json!({"city":"berlin"})) {
        Ok(res) => res,
        Err(e) => e.to_string()
    };

    println!("{}", result);
}
