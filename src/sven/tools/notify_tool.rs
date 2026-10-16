use std::sync::OnceLock;

use tokio::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::tools::subprocess;

/// The project logo, embedded at compile time so notifications carry the
/// sven icon no matter where the binary runs from. `notify-send --icon`
/// takes a filename or icon name, not image bytes, so it is written to a
/// temp file once per process.
const LOGO_SVG: &[u8] = include_bytes!("../../../assets/logo.svg");

/// Write the embedded logo to a temp file and return its path.
///
/// The file is written at most once per process (the `OnceLock` caches
/// the result, including a `None` failure): the logo is embedded in the
/// binary, so it cannot change while the process runs, and rewriting it
/// on every send would only burn SSD write cycles. A newer binary
/// simply embeds a newer logo and writes it on its first send. Returns
/// `None` if the write fails — a missing icon must not block the
/// notification.
fn logo_icon_path() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let path = std::env::temp_dir().join("sven-logo.svg");
            std::fs::write(&path, LOGO_SVG).ok()?;
            Some(path.to_string_lossy().into_owned())
        })
        .clone()
}

#[derive(Deserialize, Debug, JsonSchema)]
struct NotifySendToolParams {
    /// The summary (title) of the notification.
    summary: String,
    /// The body text of the notification.
    body: Option<String>,
    /// Urgency level: low, normal or critical.
    urgency: Option<Urgency>,
    /// How long the notification stays on screen, in milliseconds.
    /// Some desktops (GNOME Shell, Notify OSD) ignore this.
    expire_time: Option<u32>,
    /// Name of the application shown as the notification's origin.
    app_name: Option<String>,
}

/// An enum instead of a free-form string: the urgency value is passed to
/// `notify-send` as `-u LEVEL`, and a typo or injection attempt would
/// otherwise surface as a confusing daemon error (or, worse, be parsed
/// as another option). Deserialization rejects anything but the three
/// spec levels up front.
#[derive(Deserialize, Debug, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Urgency {
    Low,
    Normal,
    Critical,
}

/// Build the `notify-send` invocation for the given parameters. `icon`
/// is the path of the logo temp file, if writing it succeeded.
///
/// Split out from the tool body so the argv layout is unit-testable
/// without a notification daemon installed.
fn build_command(params: &NotifySendToolParams, icon: Option<String>) -> Command {
    let mut command = Command::new("notify-send");
    if let Some(urgency) = &params.urgency {
        command.arg("--urgency").arg(match urgency {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::Critical => "critical",
        });
    }
    if let Some(expire_time) = params.expire_time {
        command.arg("--expire-time").arg(expire_time.to_string());
    }
    if let Some(app_name) = &params.app_name {
        command.arg("--app-name").arg(app_name);
    }
    if let Some(icon) = icon {
        command.arg("--icon").arg(icon);
    }
    // `--` stops option parsing: a summary or body starting with `-`
    // (e.g. "-t 5000") must reach notify-send as text, never as one of
    // its own options — `-A` would even make it block waiting for user
    // input.
    command.arg("--").arg(&params.summary);
    if let Some(body) = &params.body {
        command.arg(body);
    }
    command
}

tool!(NotifySendTool, NotifySendToolParams, "Send a desktop notification to the user via notify-send. Takes a summary (title) and an optional body, plus optional urgency (low/normal/critical), expire time in milliseconds and app name.", execute(args) {
    if args.summary.trim().is_empty() {
        return Err("notification summary must not be empty".into());
    }
    let mut command = build_command(&args, logo_icon_path());
    // notify-send prints nothing on success; `run` still surfaces a
    // non-zero exit (e.g. no notification daemon running) as an error.
    subprocess::run(&mut command).await?;
    Ok("notification sent".to_string())
});

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(command: &Command) -> Vec<String> {
        command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn params(summary: &str) -> NotifySendToolParams {
        NotifySendToolParams {
            summary: summary.to_string(),
            body: None,
            urgency: None,
            expire_time: None,
            app_name: None,
        }
    }

    #[test]
    fn dash_leading_summary_lands_after_the_option_terminator() {
        // option-shaped text must reach notify-send as the summary, not
        // as its own options
        let command = build_command(&params("-t 5000"), None);
        assert_eq!(args_of(&command), vec!["--", "-t 5000"]);
    }

    #[test]
    fn all_options_are_passed_in_order() {
        let command = build_command(
            &NotifySendToolParams {
                summary: "done".to_string(),
                body: Some("build finished".to_string()),
                urgency: Some(Urgency::Critical),
                expire_time: Some(5000),
                app_name: Some("sven".to_string()),
            },
            Some("logo.svg".to_string()),
        );
        assert_eq!(
            args_of(&command),
            vec![
                "--urgency",
                "critical",
                "--expire-time",
                "5000",
                "--app-name",
                "sven",
                "--icon",
                "logo.svg",
                "--",
                "done",
                "build finished"
            ]
        );
    }

    #[test]
    fn minimal_notification_is_summary_only() {
        let command = build_command(&params("hello"), None);
        assert_eq!(args_of(&command), vec!["--", "hello"]);
    }

    #[test]
    fn icon_is_passed_when_available() {
        let command = build_command(&params("hello"), Some("logo.svg".to_string()));
        assert_eq!(args_of(&command), vec!["--icon", "logo.svg", "--", "hello"]);
    }

    #[test]
    fn logo_is_written_once_with_the_embedded_bytes() {
        // first call writes the temp file with the embedded logo
        let path = logo_icon_path().expect("temp dir must be writable");
        let bytes = std::fs::read(&path).expect("logo temp file must be readable");
        assert_eq!(bytes, LOGO_SVG);
        assert!(std::str::from_utf8(&bytes).unwrap().starts_with("<svg"));

        // the OnceLock caches the path, so a second call must not rewrite
        // the file: delete it, call again, and it must stay gone. Kept in
        // the same test as the write check above — both share the
        // process-global cache, and separate tests would race under the
        // default multi-threaded test harness.
        std::fs::remove_file(&path).expect("logo temp file must be deletable");
        assert_eq!(logo_icon_path(), Some(path.clone()));
        assert!(!std::path::Path::new(&path).exists(), "cached path must not be rewritten");
    }

    // End-to-end through Tool::execute, not just the helpers: this stays
    // green on machines without notify-send because the empty summary is
    // rejected before the binary is ever spawned.
    #[tokio::test]
    async fn execute_rejects_empty_summary() {
        use crate::sven::tool::Tool;
        let err = NotifySendTool
            .execute(serde_json::json!({ "summary": "   " }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }
}
