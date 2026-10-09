use tokio::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::tools::subprocess;

#[derive(Deserialize, Debug, JsonSchema)]
struct ManPageToolParams {
    /// name of the man page to display
    name: String,
}

/// Validate a man page name before it reaches the `man` binary.
///
/// `man` reads a leading `-` as one of its own options, and any argument
/// containing `/` as a *file path* — `man /etc/passwd` prints the file,
/// bypassing the path confinement every file tool enforces. No legitimate
/// page name starts with `-` or contains `/`, so both are rejected
/// (the slash check also covers `../` traversal, which always contains
/// a slash). Failing closed here keeps the tool a man-page reader, not
/// an arbitrary-file reader.
fn validate_name(name: &str) -> Result<(), String> {
    if name.starts_with('-') {
        return Err(format!(
            "invalid man page name {name}: page names never start with '-'"
        ));
    }
    if name.contains('/') {
        return Err(format!(
            "invalid man page name {name}: page names never contain '/'"
        ));
    }
    Ok(())
}

tool!(ManPageTool, ManPageToolParams, "Displays the first page of a manual page.", execute(args) {
    validate_name(&args.name)?;
    let mut command = Command::new("man");
    command.arg(args.name);
    Ok(subprocess::run(&mut command).await?)
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_option_and_path_shaped_names() {
        // leading dash: man would parse these as its own options
        assert!(validate_name("-S").is_err());
        assert!(validate_name("--help").is_err());
        // slash: man would open the file instead of looking up a page —
        // this is the arbitrary-file-read bypass (review round 3, C1)
        assert!(validate_name("/etc/passwd").is_err());
        assert!(validate_name("src/main.rs").is_err());
        assert!(validate_name("../secrets").is_err());
        assert!(validate_name("sub/dir/page").is_err());
    }

    #[test]
    fn accepts_real_page_names() {
        assert!(validate_name("ls").is_ok());
        assert!(validate_name("man-db").is_ok());
        assert!(validate_name("systemd.service").is_ok());
        assert!(validate_name("3printf").is_ok());
    }

    // End-to-end through Tool::execute, not just validate_name(): the
    // unit tests above would stay green if the validate_name() call were
    // ever removed from the tool body. Asserting on the message (not
    // merely "some error") keeps this honest on machines without `man`
    // installed, where a bypassed validation would fail at spawn with a
    // different error text.
    #[tokio::test]
    async fn execute_rejects_path_shaped_names() {
        use crate::sven::tool::Tool;
        let err = ManPageTool
            .execute(serde_json::json!({ "name": "/etc/passwd" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("never contain '/'"));
    }
}