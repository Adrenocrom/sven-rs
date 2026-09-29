//! Terminal output helpers.
//!
//! ANSI colors are used only when stdout is a terminal and `NO_COLOR` is
//! not set (https://no-color.org), so piped output stays clean.

use std::io::IsTerminal;
use std::sync::OnceLock;

static COLOR: OnceLock<bool> = OnceLock::new();

pub fn enabled() -> bool {
    *COLOR.get_or_init(|| {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
    })
}

/// Green — tool-call headers.
pub fn green(text: &str) -> String {
    if enabled() {
        format!("\x1b[32m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Red — errors.
pub fn red(text: &str) -> String {
    if enabled() {
        format!("\x1b[31m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Bold — the token-usage line.
pub fn bold(text: &str) -> String {
    if enabled() {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Escape sequence opening the "thinking" color.
pub fn thinking() -> &'static str {
    if enabled() {
        "\x1b[38;2;10;140;75m"
    } else {
        ""
    }
}

/// Escape sequence resetting the color.
pub fn reset() -> &'static str {
    if enabled() {
        "\x1b[0m"
    } else {
        ""
    }
}