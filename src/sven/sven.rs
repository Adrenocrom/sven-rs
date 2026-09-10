use std::io::BufReader;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatOptions {
    pub temperature: f32,
    pub num_ctx: i32,
    pub repeat_penalty: f32,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            temperature: 0.1,
            num_ctx: 32000,
            repeat_penalty: 1.2,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct SvenConfig {
    pub model: String,
    pub host: String,
    pub system_prompt: String,
    pub options: ChatOptions,
}

impl Default for SvenConfig {
    fn default() -> Self {
        Self {
            model: "gemma4:12b".to_string(),
            host: "http://localhost:11434".to_string(),
            system_prompt: "".to_string(),
            options: ChatOptions::default(),
        }
    }
}

impl SvenConfig {
    pub fn load() -> SvenConfig {
        let file = match std::fs::File::open("/home/josef/.config/sven/sven.json") {
            Ok(file) => file,
            Err(e) => {
                println!("Error opening file: {}", e);
                return SvenConfig::default();
            }
        };

        let reader = BufReader::new(file);

        let config: SvenConfig = match serde_json::from_reader(reader) {
            Ok(config) => config,
            Err(e) => {
                println!("Error parsing JSON: {}", e);
                return SvenConfig::default();
            }
        };
        config
    }
}
