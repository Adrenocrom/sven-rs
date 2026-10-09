use tokio::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::tools::subprocess;

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

/// Build the `notify-send` invocation for the given parameters.
///
/// Split out from the tool body so the argv layout is unit-testable
/// without a notification daemon installed.
fn build_command(params: &NotifySendToolParams) -> Command {
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
    let mut command = build_command(&args);
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
        let command = build_command(&params("-t 5000"));
        assert_eq!(args_of(&command), vec!["--", "-t 5000"]);
    }

    #[test]
    fn all_options_are_passed_in_order() {
        let command = build_command(&NotifySendToolParams {
            summary: "done".to_string(),
            body: Some("build finished".to_string()),
            urgency: Some(Urgency::Critical),
            expire_time: Some(5000),
            app_name: Some("sven".to_string()),
        });
        assert_eq!(
            args_of(&command),
            vec![
                "--urgency",
                "critical",
                "--expire-time",
                "5000",
                "--app-name",
                "sven",
                "--",
                "done",
                "build finished"
            ]
        );
    }

    #[test]
    fn minimal_notification_is_summary_only() {
        let command = build_command(&params("hello"));
        assert_eq!(args_of(&command), vec!["--", "hello"]);
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