use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct FindToolParams {
    /// Unix shell‑style wildcard pattern (e.g. "*.py")
    pattern: String
}
tool!(FindTool, FindToolParams, "Search for files whose names match *pattern*.", execute(args) {
    let mut command = Command::new("find");
    command.arg(".");
    command.arg("-name");
    command.arg(args.pattern);
    let output = command.output()?;
    let out = String::from_utf8(output.stdout)?;
    Ok(out.trim().to_string())
});
