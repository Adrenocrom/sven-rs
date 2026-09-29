use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::tools::subprocess;

#[derive(Deserialize, Debug, JsonSchema)]
struct ManPageToolParams {
    /// name of the man page to display
    name: String,
}
tool!(ManPageTool, ManPageToolParams, "Displays the first page of a manual page.", execute(args) {
    // man would parse a leading "-" as one of its own options; no
    // legitimate page name starts with one, so fail closed
    if args.name.starts_with('-') {
        return Err(format!("invalid man page name {}: page names never start with '-'", args.name).into());
    }
    let mut command = Command::new("man");
    command.arg(args.name);
    Ok(subprocess::run(&mut command)?)
});