//! Shared subprocess plumbing for the external-binary tools
//! (`grep`, `find`, `man`, `curl`, `ddgr`, `cargo`, …).

use std::process::Command;

/// Run `command` to completion and return its stdout.
///
/// A non-zero exit status is an error that includes the program name, the
/// status and stderr — without this, a failing program (bad regex, missing
/// directory, HTTP 404) is indistinguishable from "no output", and the
/// model would confidently build on a broken result.
pub fn run(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let out = command.output()?;
    if !out.status.success() {
        return Err(format!(
            "{} exited with {}: {}",
            command.get_program().to_string_lossy(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}