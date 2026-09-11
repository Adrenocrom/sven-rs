use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{Read, Write};
use std::path::Path;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sven::macros::tool;
use crate::sven::security;
use crate::sven::tool::Tool;
use crate::sven::tool_error::ToolError;

#[derive(Deserialize, Debug, JsonSchema)]
struct SearchAndReplaceParams {
    /// path of file to operate
    path: String,
    oldcontent: String,
    newcontent: String
}
tool!(SearchAndReplaceTool, SearchAndReplaceParams, "search and replace content in a file", execute(args) {
        security::is_inside_cwd(&args.path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(args.path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        let new_content = content.replace(args.oldcontent.as_str(), args.newcontent.as_str());
        file.write_all(new_content.as_bytes())?;
        Ok("Updated successfully".to_string())
});

//impl Tool for SearchAndReplaceTool {
//    fn name(&self) -> String {
//        "SearchAndReplace".to_string()
//    }
//
//    fn description(&self) -> String {
//        "Search and replace".to_string()
//    }
//
//    fn parameters(&self) -> Option<Value> {
//        Some(json!({
//            "type": "object",
//            "required": ["path", "oldcontent", "newcontent"],
//            "properties": {
//                "path": {
//                    "type": "string",
//                    "description": "file path"
//                },
//                "oldcontent": {
//                    "type": "string",
//                    "description": "old content which will be replaced"
//                },
//                "newcontent": {
//                    "type": "string",
//                    "description": "new content"
//                },
//            }
//        }))
//    }
//
//    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
//        let path = match parameters.get("path").and_then(|p| p.as_str()) {
//            Some(p) => p,
//            None => return Err(ToolError::MissingParameter("path".to_string())),
//        };
//        let old_content = match parameters.get("oldcontent").and_then(|p| p.as_str()) {
//            Some(p) => p,
//            None => return Err(ToolError::MissingParameter("oldcontent".to_string())),
//        };
//        let new_content = match parameters.get("newcontent").and_then(|p| p.as_str()) {
//            Some(p) => p,
//            None => return Err(ToolError::MissingParameter("newcontent".to_string())),
//        };
//
//        security::is_inside_cwd(&path)?;
//
//        let mut file = OpenOptions::new()
//            .read(true)
//            .write(true)
//            .open(path)?;
//        let mut content = String::new();
//        file.read_to_string(&mut content)?;
//        let new_content = content.replace(old_content, new_content);
//        file.write_all(new_content.as_bytes())?;
//        Ok("Updated successfully".to_string())
//    }
//}

//pub struct ReplaceFileTool;
//impl Tool for ReplaceFileTool {
//    fn name(&self) -> String {
//        "replaceFile".to_string()
//    }
//
//    fn description(&self) -> String {
//        "Replace the whole file with new content. If the directory does not exist, it is created. If the file does not exists, a new file is created.".to_string()
//    }
//
//    fn parameters(&self) -> Option<Value> {
//        Some(json!({
//            "type": "object",
//            "required": ["path", "oldcontent", "newcontent"],
//            "properties": {
//                "path": {
//                    "type": "string",
//                    "description": "file path"
//                },
//                "newcontent": {
//                    "type": "string",
//                    "description": "new content which will replace all contents of the file"
//                },
//            }
//        }))
//    }
//
//    fn execute_tool(&self, parameters: Value) -> Result<String, ToolError> {
//        let path = match parameters.get("path").and_then(|p| p.as_str()) {
//            Some(p) => p,
//            None => return Err(ToolError::MissingParameter("path".to_string())),
//        };
//        let new_content = match parameters.get("newcontent").and_then(|p| p.as_str()) {
//            Some(p) => p,
//            None => return Err(ToolError::MissingParameter("newcontent".to_string())),
//        };
//        security::is_inside_cwd(&path)?;
//
//        //create_dir_all(Path::new(path).parent())?;
//
//        Ok("not implemented correctly".to_string())
//    }
//}
