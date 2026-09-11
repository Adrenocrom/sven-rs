use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::security;

#[derive(Deserialize, Debug, JsonSchema)]
struct ListFilesParams {
    /// file path
    path: Option<String>
}
tool!(ListFiles, ListFilesParams, "List files in current directory", execute(args) {
    let mut command = Command::new("ls");
    command.arg("-1");
    if let Some(path) = args.path {
        security::is_inside_cwd(&path)?;
        command.arg(path);
    }
    let output = command.output()?;
    let out = String::from_utf8(output.stdout)?;
    Ok(out)
});
