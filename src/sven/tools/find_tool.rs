use tokio::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::security;
use crate::sven::tools::subprocess;

#[derive(Deserialize, Debug, JsonSchema)]
struct FindToolParams {
    /// Unix shell‑style wildcard pattern (e.g. "*.py")
    pattern: String,
    /// Directory to search in (default: current directory)
    path: Option<String>,
}
tool!(FindTool, FindToolParams, "Search for files whose names match *pattern*.", execute(args) {
    let mut command = Command::new("find");
    if let Some(path) = args.path {
        security::is_inside_cwd(&path)?;
        // a leading "-" would be parsed as an option or expression
        // (e.g. `-delete`), not as a path — force the operand reading
        command.arg(subprocess::as_operand(&path));
    } else {
        command.arg(".");
    }
    command.arg("-name");
    command.arg(args.pattern);
    command.arg("-not");
    command.arg("-path");
    command.arg("./target/*");
    command.arg("-not");
    command.arg("-path");
    command.arg("./.git/*");
    Ok(subprocess::run(&mut command).await?.trim().to_string())
});