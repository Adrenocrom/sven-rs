#![feature(normalize_lexically)]
#![feature(path_absolute_method)]
mod sven;

use std::io::BufRead;

use clap::Parser;

use crate::sven::config::SvenConfig;
use crate::sven::tools::compile_tool::CompileTool;
use crate::sven::tools::edit_tool::{ReplaceFileTool, SearchAndReplaceTool};
use crate::sven::tools::find_tool::FindTool;
use crate::sven::tools::grep_tool::GrepTool;
use crate::sven::tools::manpage_tool::ManPageTool;
use crate::sven::tools::read_tool::ReadTool;
use crate::sven::tools::skill_tools::{
    AddSkillTool, GetSkillTool, ListSkillsTool, RemoveSkillTool, SearchSkillsTool,
    UpdateSkillTool,
};
use crate::sven::tools::web_fetch::WebFetch;
use crate::sven::tools::web_search::WebSearch;
use crate::sven::tools::{list_files::ListFiles, time_tool::TimeTool};
use sven::agent::{Agent, AgentConfig};

use crate::sven::tool_registry::ToolRegistry;
use crate::sven::skills;

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

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let config = SvenConfig::load();
    skills::init_skills_dir(&config.data_dir);
    println!("{} ({})", &config.model, &config.options.num_ctx);

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

    let mut agent = Agent::new(AgentConfig {
        host: config.host,
        model: config.model,
        system_prompt: config.system_prompt,
        options: config.options,
        tool_registry: registry,
    });

    let mut rl = rustyline::DefaultEditor::new().expect("Could not init RustyLine");
    loop {
        if let Some(end_of_prompt) = &args.end_of_prompt {
            let stdin = std::io::stdin();
            let mut user_prompt = String::new();

            for line in stdin.lock().lines() {
                match line {
                    Ok(line) => {
                        if line.contains(end_of_prompt) {
                            break;
                        }
                        user_prompt.push_str(&line);
                    },
                    Err(_) => return,
                }

                user_prompt.push('\n'); // `lines()` strips the newline, Python's readline() keeps it
            }

            let user_prompt = user_prompt.trim();
            if "/close".eq(user_prompt) {
                break;
            }
            else if "/clear".eq(user_prompt) {
                agent.clear();
            }
            else {
                agent.run(&user_prompt).await;
            }
        }
        else {
            let readline = rl.readline(&args.prompt);
            match readline {
                Ok(line) => {
                    if "/close".eq(&line) {
                        break;
                    } else if "/clear".eq(&line) {
                        agent.clear();
                    } else {
                        agent.run(&line).await;
                    }
                },
                Err(_) => {
                    break;
                }
            }
        }
    }
}
