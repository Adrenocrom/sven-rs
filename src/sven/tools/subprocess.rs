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
}