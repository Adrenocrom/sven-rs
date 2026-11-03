//! Shared subprocess plumbing for the external-binary tools
//! (`grep`, `find`, `man`, `pandoc`, `ddgr`, `cargo`, …).

use tokio::process::Command;

/// Run `command` to completion and return its stdout.
///
/// A non-zero exit status is an error that includes the program name, the
/// status and stderr — without this, a failing program (bad regex, missing
/// directory, HTTP 404) is indistinguishable from "no output", and the
/// model would confidently build on a broken result.
pub async fn run(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let out = command.output().await?;
    if !out.status.success() {
        return Err(format!(
            "{} exited with {}: {}",
            command.as_std().get_program().to_string_lossy(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `command` to completion and return its stderr.
///
/// A non-zero exit status is an error that includes the program name, the
/// status and stderr — without this, a failing program (bad regex, missing
/// directory, HTTP 404) is indistinguishable from "no output", and the
/// model would confidently build on a broken result.
pub async fn run_stderr(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let out = command.output().await?;
    if !out.status.success() {
        return Err(format!(
            "{} exited with {}: {}",
            command.as_std().get_program().to_string_lossy(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Run `command` to completion and return stdout and stderr combined.
///
/// Some programs split one logical report across both streams: `cargo test`
/// writes the test result summary and `println!` output to stdout, but
/// compile errors and panic messages to stderr. A tool that wants the whole
/// picture must merge them. stdout comes first, then stderr, so the model
/// reads the summary before the diagnostics that explain it.
///
/// A non-zero exit status is an error carrying the same combined text:
/// `cargo test` exits non-zero when tests fail, and that output is exactly
/// what the model needs to fix them.
pub async fn run_combined(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let out = command.output().await?;
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        return Err(format!(
            "{} exited with {}: {}",
            command.as_std().get_program().to_string_lossy(),
            out.status,
            combined.trim()
        )
        .into());
    }
    Ok(combined)
}

/// Return `path` in a form the called program reads as an operand, never
/// as one of its own options.
///
/// `find` and friends interpret an argv element starting with `-` as an
/// option or expression — `find -delete …` deletes everything it traverses
/// and exits 0. Prefixing `./` makes the operand reading unambiguous while
/// keeping genuinely dash-named paths working; find(1) itself recommends
/// this because its `--` handling is unreliable.
pub fn as_operand(path: &str) -> String {
    if path.starts_with('-') {
        format!("./{path}")
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_leading_paths_become_operands() {
        // option-shaped strings are turned into unambiguous operands
        assert_eq!(as_operand("-delete"), "./-delete");
        assert_eq!(as_operand("-maxdepth"), "./-maxdepth");
        assert_eq!(as_operand("-"), "./-");
        assert_eq!(as_operand("--"), "./--");
        // everything else passes through unchanged
        assert_eq!(as_operand("src"), "src");
        assert_eq!(as_operand("./-weird"), "./-weird");
        assert_eq!(as_operand("/abs/path"), "/abs/path");
    }

    #[tokio::test]
    async fn combined_merges_stdout_then_stderr() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("echo out; echo err >&2");
        let combined = run_combined(&mut command).await.unwrap();
        assert_eq!(combined, "out\nerr\n");
    }

    #[tokio::test]
    async fn combined_reports_failure_with_both_streams() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("echo out; echo boom >&2; exit 1");
        let err = run_combined(&mut command).await.unwrap_err().to_string();
        assert!(err.contains("out"), "stdout missing from error: {err}");
        assert!(err.contains("boom"), "stderr missing from error: {err}");
    }
}
