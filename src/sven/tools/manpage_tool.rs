use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct ManPageToolParams {
    /// name of the man page to display
    name: String
}
tool!(ManPageTool, ManPageToolParams, "Displays the first page of a manual page. Usage: manpage <name>", execute(args) {
    let mut command = Command::new("man");
    command.arg(args.name);
    let output = command.output()?;
    let out = String::from_utf8(output.stdout)?;
    Ok(out.to_string())
});
