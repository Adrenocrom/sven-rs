use tokio::process::Command;

use crate::sven::{macros::tool, tools::subprocess};

tool!(GitDiffTool, "get Git diff", execute() {
    let mut command = Command::new("git");
    command.arg("diff");
    command.arg(".");
    Ok(subprocess::run(&mut command).await?)
});

