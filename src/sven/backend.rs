use serde::Deserialize;

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub enum Backend {
    Ollama,
    OpenAI, // API key lives here
}

impl Backend {
    pub fn endpoint(backend: &Backend, host: &String) -> String {
        match backend {
            Backend::Ollama => format!("{}/api/chat", &host),
            Backend::OpenAI => format!("{}/v1/chat/completions", &host),
        }
    }
}

impl ToString for Backend {
    fn to_string(&self) -> String {
        match self {
            Backend::Ollama => "ollama".to_string(),
            Backend::OpenAI => "openai".to_string(),
        }
    }
}

impl From<String> for Backend {
    fn from(value: String) -> Self {
        match value.as_str() {
            "openai" => Backend::OpenAI,
            _ =>  Backend::Ollama, // Default if not supported by the string
        }
    }
}
