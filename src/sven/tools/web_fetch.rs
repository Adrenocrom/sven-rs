use std::process::{Command, Stdio};

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct WebFetchParams {
    /// URL starting with http:// or https://
    url: String,
}
tool!(WebFetch, WebFetchParams, "Does a GET request to a specified URL.", execute(args) {
    let curl_output = match Command::new("curl")
        .arg("--silent")
        .arg("-L")
        .arg("-f")
        .arg("--")
        .arg(args.url)
        .stdout(Stdio::piped())
        .spawn()?
        .stdout
        {
            Some(stdout) => stdout,
            None => return Ok("".to_string()),
        };

    let pandoc_output = Command::new("pandoc")
        .arg("-f")
        .arg("html")
        .arg("-t")
        .arg("gfm")
        .stdin(Stdio::from(curl_output))
        .stdout(Stdio::piped())
        .spawn()?
        .wait_with_output()?;

    let result = String::from_utf8(pandoc_output.stdout)?;
    Ok(result)
});
