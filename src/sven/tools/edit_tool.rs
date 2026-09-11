use std::fs::{self, OpenOptions};
use std::io::{Read, Write};

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::security;

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

#[derive(Deserialize, Debug, JsonSchema)]
pub struct ReplaceFileToolParams {
    /// file path
    path: String,
    /// new content which will replace all contents of the file
    newcontent: String,
}

tool!(ReplaceFileTool, ReplaceFileToolParams, "Replace the whole file with new content. If the file does not exists, a new file is created.", execute(args) {
    security::is_inside_cwd(&args.path)?;
    fs::write(&args.path, &args.newcontent)?;
    Ok("Success".to_string())
});
