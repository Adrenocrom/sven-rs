use tokio::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::tools::subprocess;

#[derive(Deserialize, Debug, JsonSchema)]
struct WebSearchParams {
    /// The search string to submit to DuckDuckGo. This can be a simple keyword, a phrase, or a more elaborate Boolean expression (e.g., 'site:example.com "error code"').
    query: String,
}
tool!(WebSearch, WebSearchParams, "Search the web via DuckDuckGo. Use WebFetch for further investigation.", execute(args) {
    let mut command = Command::new("ddgr");
    command.arg("--noprompt");
    // end-of-options marker: the query is a keyword list, never options
    command.arg("--");
    command.arg(args.query);
    Ok(subprocess::run(&mut command).await?)
});