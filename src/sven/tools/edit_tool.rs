use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

use schemars::JsonSchema;
use serde::{Deserialize};

use crate::sven::macros::tool;
use crate::sven::security;

#[derive(Deserialize, Debug, JsonSchema)]
struct SearchAndReplaceParams {
    /// path of file to operate
    path: String,
    /// the content to be replaced
    oldcontent: String,
    /// the content to be replaced with
    newcontent: String,
    // type of replacing method, one of First, All and Last
    n: Option<usize>,
}

tool!(SearchAndReplaceTool, SearchAndReplaceParams, "search and replace content in a file", execute(args) {
    security::is_inside_cwd(&args.path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&args.path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    let new_content : String = match args.n {
        Some(n) => content.replacen(args.oldcontent.as_str(), args.newcontent.as_str(), n),
        None => content.replace(args.oldcontent.as_str(), args.newcontent.as_str()),
    };
    println!("{}", new_content);
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(new_content.as_bytes())?;
    Ok(format!("File {} replaced successfully", &args.path))
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
    Ok(format!("File {} replaced successfully", &args.path))
});
