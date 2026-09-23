#![feature(normalize_lexically)]
#![feature(path_absolute_method)]
mod sven;

use std::io::Write;

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

#[tokio::main]
async fn main() {
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

    loop {
        print!("\n> ");
        let _ = std::io::stdout().flush();
        let mut input = String::new();
        // read_line returns Ok(0) on EOF (Ctrl-D); treating that as an empty
        // line would loop forever sending empty prompts to the model.
        match std::io::stdin().read_line(&mut input) {
            Ok(0) => break, // EOF
            Ok(_) => {
                if "/close\n".eq(&input) {
                    break;
                } else if "/clear\n".eq(&input) {
                    agent.clear();
                } else {
                    agent.run(&input).await;
                }
            }
            Err(e) => {
                eprintln!("cannot read input: {}", e);
                break;
            }
        }
    }
}
