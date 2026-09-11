use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct WebSearchParams {
    /// The search string to submit to DuckDuckGo. This can be a simple keyword, a phrase, or a more elaborate Boolean expression (e.g., 'site:example.com "error code"').
    query: String,
}
tool!(WebSearch, WebSearchParams, "Search the web via DuckDuckGo. Use WebFetch for further investion" , execute(args) {
    let mut command = Command::new("ddgr");
    command.arg("--noprompt");
    command.arg(args.query);
    let output = command.output()?;
    let out = String::from_utf8(output.stdout)?;
    Ok(out)
});
