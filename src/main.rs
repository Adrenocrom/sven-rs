mod sven;

use std::io::BufRead;

use clap::Parser;

use crate::sven::backend::Backend;
use crate::sven::config::SvenConfig;
use crate::sven::skills;
use crate::sven::tool_registry::ToolRegistry;
use crate::sven::tools::compile_tool::CompileTool;
use crate::sven::tools::edit_tool::{ReplaceFileTool, SearchAndReplaceTool};
use crate::sven::tools::find_tool::FindTool;
use crate::sven::tools::grep_tool::GrepTool;
use crate::sven::tools::list_files::ListFiles;
use crate::sven::tools::manpage_tool::ManPageTool;
use crate::sven::tools::read_tool::ReadTool;
use crate::sven::tools::skill_tools::{
    AddSkillTool, GetSkillTool, ListSkillsTool, RemoveSkillTool, SearchSkillsTool,
    UpdateSkillTool,
};
use crate::sven::tools::time_tool::TimeTool;
use crate::sven::tools::web_fetch::WebFetch;
use crate::sven::tools::web_search::WebSearch;
use sven::agent::{Agent, AgentConfig};

/// Sven is a simple command line ai agent
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// the prefix to be used as a prompt for the AI
    #[arg(short, long, default_value_t = ">> ".to_string() )]
    prompt: String,

    /// if this is set, stdin reads until the value of `end_of_prompt is recieved`
    #[arg(long)]
    end_of_prompt: Option<String>
}

fn build_agent(config: SvenConfig, api_key: Option<String>) -> Agent {
    let mut registry: ToolRegistry = ToolRegistry::new();
    registry.register(Box::new(TimeTool));
    registry.register(Box::new(ListFiles));
    registry.register(Box::new(ReadTool));
    registry.register(Box::new(WebSearch));
    registry.register(Box::new(WebFetch));
    registry.register(Box::new(SearchAndReplaceTool));
    registry.register(Box::new(ReplaceFileTool));
    registry.register(Box::new(GrepTool));
    registry.register(Box::new(FindTool));
    registry.register(Box::new(ManPageTool));
    registry.register(Box::new(AddSkillTool));
    registry.register(Box::new(UpdateSkillTool));
    registry.register(Box::new(RemoveSkillTool));
    registry.register(Box::new(ListSkillsTool));
    registry.register(Box::new(SearchSkillsTool));
    registry.register(Box::new(GetSkillTool));
    registry.register(Box::new(CompileTool));

    Agent::new(AgentConfig {
        host: config.host,
        model: config.model,
        system_prompt: config.system_prompt,
        options: config.options,
        tool_registry: registry,
        backend: config.backend,
        api_key,
    })
}

/// Read one prompt from stdin until a line containing `end_of_prompt`.
/// Returns `None` at EOF when no input is pending. On the sentinel line
/// only the text before the marker is kept.
fn read_prompt(end_of_prompt: &str) -> Option<String> {
    let stdin = std::io::stdin();
    let mut user_prompt = String::new();
    let mut received = false;

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => return None,
        };
        received = true;
        match line.find(end_of_prompt) {
            Some(pos) => {
                user_prompt.push_str(&line[..pos]);
                user_prompt.push('\n');
                break;
            }
            None => {
                user_prompt.push_str(&line);
                user_prompt.push('\n'); // `lines()` strips the newline
            }
        }
    }

    if !received {
        return None;
    }
    Some(user_prompt.trim().to_string())
}

fn print_header(config: &SvenConfig) {
    println!("{}", config.backend.to_string());
    // `num_ctx` is an Ollama-only option — OpenAI-compatible servers
    // (openai, vllm) ignore it, so it is not shown for them
    match (&config.backend, config.options.num_ctx) {
        (Backend::Ollama, Some(num_ctx)) => println!("{} ({})\n", &config.model, &num_ctx),
        _ => println!("{}\n", &config.model),
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let config = SvenConfig::load();
    skills::init_skills_dir(&config.data_dir);
    print_header(&config);

    // The API key never goes in the config file — it is read from the
    // environment so it cannot leak through file reads or backups.
    let api_key = std::env::var("SVEN_API_KEY").ok().filter(|key| !key.is_empty());
    let mut agent = build_agent(config.clone(), api_key);

    if let Some(end_of_prompt) = &args.end_of_prompt {
        // non-interactive mode: process prompts from stdin until EOF
        while let Some(user_prompt) = read_prompt(end_of_prompt) {
            match user_prompt.as_str() {
                "/close" => return,
                "/clear" => agent.clear(),
                "" => continue,
                _ => agent.run(&user_prompt).await,
            }
        }
        return;
    }

    // interactive REPL
    let mut rl = rustyline::DefaultEditor::new().expect("Could not init RustyLine");
    let history_path = skills::expand_tilde(&config.data_dir).join("history");
    if let Err(_) = rl.load_history(&history_path) {
        //eprintln!("no history to load: {}", e);
    }

    loop {
        let readline = rl.readline(&args.prompt);
        match readline {
            Ok(line) => {
                let line = line.trim();
                let _ = rl.add_history_entry(line);
                if "/close".eq(line) {
                    break;
                } else if "/clear".eq(line) {
                    agent.clear();
                } else {
                    agent.run(line).await;
                }
            },
            Err(rustyline::error::ReadlineError::Interrupted) => {
                // Ctrl-C clears the line and re-prompts instead of exiting
                continue;
            },
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("readline error: {}", e);
                break;
            }
        }
    }

    if let Err(e) = rl.save_history(&history_path) {
        eprintln!("could not save history: {}", e);
    }
}
