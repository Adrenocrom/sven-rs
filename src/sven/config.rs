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

#[derive(Serialize, Deserialize, Debug, Clone)]
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
    /// `None`. A missing file is not an error: the directory is created and
    /// the defaults written there as a starting point to edit. Fields
    /// missing from an existing file fall back to the defaults field by
    /// field; a file that exists but cannot be read or parsed is reported
    /// on stderr and left untouched.
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
            // no config file yet — write one from the defaults so there
            // is something to edit
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return SvenConfig::create_default(&path);
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

    /// Write the built-in defaults to `path` so the user has a file to
    /// edit. Only called when no config file exists — an existing file is
    /// never overwritten. A failure to write is reported but not fatal:
    /// the defaults still work for this session.
    fn create_default(path: &Path) -> SvenConfig {
        let config = SvenConfig::default();
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("cannot create {}: {}", dir.display(), e);
                return config;
            }
        }
        let json = match serde_json::to_string_pretty(&config) {
            Ok(json) => json,
            Err(e) => {
                eprintln!("cannot serialize the default config: {}", e);
                return config;
            }
        };
        match std::fs::write(path, json + "\n") {
            Ok(()) => eprintln!("created default config: {}", path.display()),
            Err(e) => eprintln!("cannot write {}: {}", path.display(), e),
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Unique temp dir per test label; removed first so re-runs are clean.
    fn temp_config_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sven-config-{}-{}",
            std::process::id(),
            label
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

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

    #[test]
    fn missing_config_dir_gets_a_default_file() {
        let dir = temp_config_dir("missing");
        let config = SvenConfig::load(Some(dir.join("nested").to_str().unwrap()));
        // the defaults are used for this session
        assert_eq!(config.model, "gemma4:12b");
        assert_eq!(config.backend, Backend::Ollama);
        // and written to disk as a starting point to edit
        let file = dir.join("nested").join("sven.json");
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.contains("\"backend\": \"ollama\""));
        assert!(written.contains("\"model\": \"gemma4:12b\""));
        // the written file must parse back to the same config
        let reloaded: SvenConfig = serde_json::from_str(&written).unwrap();
        assert_eq!(reloaded.model, config.model);
        assert_eq!(reloaded.backend, config.backend);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_config_file_is_not_overwritten() {
        let dir = temp_config_dir("existing");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sven.json"), r#"{"model": "llama3"}"#).unwrap();
        let config = SvenConfig::load(Some(dir.to_str().unwrap()));
        assert_eq!(config.model, "llama3");
        // the user's file is untouched — no defaults were merged in
        let written = std::fs::read_to_string(dir.join("sven.json")).unwrap();
        assert_eq!(written, r#"{"model": "llama3"}"#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_config_file_is_not_overwritten() {
        // a file that exists but does not parse must not be replaced by
        // the defaults — the user may still want to fix it
        let dir = temp_config_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sven.json"), "not json").unwrap();
        let config = SvenConfig::load(Some(dir.to_str().unwrap()));
        assert_eq!(config.model, "gemma4:12b");
        assert_eq!(
            std::fs::read_to_string(dir.join("sven.json")).unwrap(),
            "not json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
