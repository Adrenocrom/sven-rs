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
    // Only http(s) is allowed: curl would otherwise happily fetch file://,
    // gopher://, ftp:// … and read local files, bypassing the path
    // confinement that ReadTool enforces.
    if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
        return Err(format!("unsupported URL scheme (only http/https): {}", args.url).into());
    }

    // curl is spawned with std::process so its stdout stays a plain std
    // handle — that is what `Stdio::from` needs to wire it to pandoc's
    // stdin; tokio's ChildStdout cannot be converted back. pandoc is
    // spawned with tokio and awaited, so the long part of the fetch
    // (curl downloading, pandoc converting) never blocks a worker.
    let mut curl = Command::new("curl")
        .arg("--silent")
        .arg("--show-error")
        .arg("-L")
        .arg("-f")
        .arg("--max-time")
        .arg("30")
        .arg("--")
        .arg(&args.url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let curl_stdout = curl
        .stdout
        .take()
        .ok_or_else(|| "curl stdout was not piped".to_string())?;

    let pandoc = tokio::process::Command::new("pandoc")
        .arg("-f")
        .arg("html")
        .arg("-t")
        .arg("gfm")
        .stdin(Stdio::from(curl_stdout))
        .stdout(Stdio::piped())
        .spawn()?;
    let pandoc_output = pandoc.wait_with_output().await?;

    // Waiting for pandoc first cannot deadlock: pandoc keeps draining
    // curl's stdout, and its EOF means curl has already exited — so the
    // blocking wait below returns immediately, it only reaps the status.
    let curl_output = curl.wait_with_output()?;
    if !curl_output.status.success() {
        return Err(format!(
            "curl exited with {}: {}",
            curl_output.status,
            String::from_utf8_lossy(&curl_output.stderr).trim()
        )
        .into());
    }
    if !pandoc_output.status.success() {
        return Err(format!(
            "pandoc exited with {}: {}",
            pandoc_output.status,
            String::from_utf8_lossy(&pandoc_output.stderr).trim()
        )
        .into());
    }

    Ok(String::from_utf8(pandoc_output.stdout)?)
});