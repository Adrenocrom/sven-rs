use tokio::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::security;
use crate::sven::tools::subprocess;

#[derive(Deserialize, Debug, JsonSchema)]
struct ListFilesParams {
    /// file path
    path: Option<String>,
}
tool!(ListFiles, ListFilesParams, "List files in current directory", execute(args) {
    let mut command = Command::new("ls");
    command.arg("-1");
    if let Some(path) = args.path {
        security::is_inside_cwd(&path)?;
        // end-of-options marker: a path like "-la" is an operand
        command.arg("--");
        command.arg(path);
    }
    Ok(subprocess::run(&mut command).await?)
});