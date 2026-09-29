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
    let mut command = Command::new("man");
    command.arg(args.name);
    Ok(subprocess::run(&mut command)?)
});