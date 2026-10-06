use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sven::backend::Backend;

/// Options passed to the Ollama server with each request. Fields missing
/// from the config file fall back to these defaults (`#[serde(default)]`
/// takes them from the `Default` impl).
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ChatOptions {
    pub temperature: f32,
    pub num_ctx: Option<i32>,
    pub max_tokens: Option<i32>
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            temperature: 0.1,
            num_ctx: Some(32000),
            max_tokens: None,
        }
    }
}

/// One MCP server entry from `sven.json`.
///
/// `command` (+ `args`, `env`) selects the stdio transport, `url` the
/// Streamable HTTP one; exactly one of them must be set. `env` adds to
/// the inherited environment, it does not replace it.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct McpServerConfig {
    /// Executable to run (stdio transport).
    pub command: Option<String>,
    /// Arguments for `command`.
    pub args: Option<Vec<String>>,
    /// Extra environment variables for `command`.
    pub env: Option<BTreeMap<String, String>>,
    /// Server URL (Streamable HTTP transport).
    pub url: Option<String>,
    /// Extra HTTP headers, e.g. `Authorization`.
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct SvenConfig {
    pub backend: Backend,
    pub data_dir: String,
    pub model: String,
    pub host: String,
    pub system_prompt: String,
    pub options: ChatOptions,
    /// MCP servers to connect to at startup; the key is the server name
    /// used in the tool names (`mcp__<name>__<tool>`).
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

impl Default for SvenConfig {
    fn default() -> Self {
        Self {
            data_dir: "~/.config/sven".to_string(),
            model: "gemma4:12b".to_string(),
            host: "http://localhost:11434".to_string(),
            system_prompt: "".to_string(),
            options: ChatOptions::default(),
            backend: Backend::Ollama,
            mcp_servers: BTreeMap::new(),
        }
    }
}

impl SvenConfig {
    /// Load `sven.json` from `dir`, or from `~/.config/sven` when `dir` is
    /// `None`. Missing fields — or a missing file — fall back to the
    /// defaults field by field; a file that exists but cannot be read or
    /// parsed is reported on stderr.
    pub fn load(dir: Option<&str>) -> SvenConfig {
        let path = match dir {
            // `~` in a CLI-supplied directory is expanded like in `data_dir`
            Some(dir) => crate::sven::skills::expand_tilde(dir).join("sven.json"),
            None => {
                let Some(home) = std::env::var_os("HOME") else {
                    eprintln!("HOME is not set; using default config");
                    return SvenConfig::default();
                };
                Path::new(&home).join(".config").join("sven").join("sven.json")
            }
        };
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            // no config file yet — the defaults are not an error
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return SvenConfig::default();
            }
            Err(e) => {
                eprintln!("cannot open {}: {}", path.display(), e);
                return SvenConfig::default();
            }
        };

        match serde_json::from_reader(BufReader::new(file)) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Error parsing {}: {}", path.display(), e);
                SvenConfig::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_keeps_defaults_for_missing_fields() {
        let config: SvenConfig = serde_json::from_str(r#"{"model": "llama3"}"#).unwrap();
        assert_eq!(config.model, "llama3");
        assert_eq!(config.host, "http://localhost:11434");
        assert_eq!(config.options.num_ctx, Some(32000));

        let config: SvenConfig =
            serde_json::from_str(r#"{"options": {"temperature": 0.5}}"#).unwrap();
        assert_eq!(config.options.temperature, 0.5);
        assert_eq!(config.options.num_ctx, Some(32000));
    }

    #[test]
    fn backend_field_parses_lowercase_names() {
        // the value the program itself prints must round-trip
        let config: SvenConfig = serde_json::from_str(r#"{"backend": "vllm"}"#).unwrap();
        assert_eq!(config.backend, Backend::Vllm);

        let config: SvenConfig = serde_json::from_str(r#"{"backend": "openai"}"#).unwrap();
        assert_eq!(config.backend, Backend::OpenAI);

        let config: SvenConfig = serde_json::from_str(r#"{"backend": "ollama"}"#).unwrap();
        assert_eq!(config.backend, Backend::Ollama);
    }

    #[test]
    fn unknown_backend_is_a_parse_error_not_a_silent_fallback() {
        assert!(serde_json::from_str::<SvenConfig>(r#"{"backend": "llamacpp"}"#).is_err());
    }
}
