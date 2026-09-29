use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::security;
use crate::sven::tools::subprocess;

#[derive(Deserialize, Debug, JsonSchema)]
struct GrepToolParams {
    /// Regular expression pattern to search for.
    pattern: String,
    /// Directory or file to search in (default: current directory)
    path: Option<String>,
}
tool!(GrepTool, GrepToolParams, "Search for a regex pattern recursively in the current directory (or in the given path). Excludes target/ and .git.", execute(args) {
    let mut command = Command::new("grep");
    command.arg("-rni");
    command.arg("--exclude-dir=target");
    command.arg("--exclude-dir=.git");
    command.arg("--");
    command.arg(args.pattern);
    if let Some(path) = args.path {
        security::is_inside_cwd(&path)?;
        command.arg(path);
    } else {
        command.arg(".");
    }
    Ok(subprocess::run(&mut command)?)
});