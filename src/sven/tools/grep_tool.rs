use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct GrepToolParams {
    /// Regular expression pattern to search for.
    pattern: String,
}
tool!(GrepTool, GrepToolParams, "Search for a regex pattern in given files or stdin.", execute(args) {
        let mut command = Command::new("grep");
        command.arg("-rni");
        command.arg(args.pattern);
        let output = command.output()?;
        let out = String::from_utf8(output.stdout)?;
        Ok(out)
});
