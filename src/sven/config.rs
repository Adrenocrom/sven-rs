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
            max_tokens: Some(1024),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct SvenConfig {
    pub backend: Backend,
    pub data_dir: String,
    pub model: String,
    pub host: String,
    pub api_key: Option<String>,
    pub system_prompt: String,
    pub options: ChatOptions,
}

impl Default for SvenConfig {
    fn default() -> Self {
        Self {
            data_dir: "~/.config/sven".to_string(),
            model: "gemma4:12b".to_string(),
            host: "http://localhost:11434".to_string(),
            api_key: None,
            system_prompt: "".to_string(),
            options: ChatOptions::default(),
            backend: Backend::Ollama
        }
    }
}

impl SvenConfig {
    /// Load `~/.config/sven/sven.json`. Missing fields — or a missing file
    /// — fall back to the defaults field by field; a file that exists but
    /// cannot be read or parsed is reported on stderr.
    pub fn load() -> SvenConfig {
        let Some(home) = std::env::var_os("HOME") else {
            eprintln!("HOME is not set; using default config");
            return SvenConfig::default();
        };
        let path = Path::new(&home).join(".config").join("sven").join("sven.json");
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
}
