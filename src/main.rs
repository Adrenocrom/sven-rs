#![feature(normalize_lexically)]
#![feature(path_absolute_method)]
mod sven;

use std::io::Write;

use crate::sven::sven::SvenConfig;
use crate::sven::tools::edit_tool::{ReplaceFileTool, SearchAndReplaceTool};
use crate::sven::tools::find_tool::FindTool;
use crate::sven::tools::grep_tool::GrepTool;
use crate::sven::tools::manpage_tool::ManPageTool;
use crate::sven::tools::read_tool::ReadTool;
use crate::sven::tools::web_fetch::WebFetch;
use crate::sven::tools::web_search::WebSearch;
use crate::sven::tools::{list_files::ListFiles, time_tool::TimeTool};
use sven::agent::{Agent, AgentConfig};

use crate::sven::tool::Tool;
use crate::sven::tool_registry::ToolRegistry;

#[tokio::main]
async fn main() {
    let config = SvenConfig::load();
    println!("{}", config.model);
    println!("{}", config.options.num_ctx);

    let time_tool: Box<dyn Tool> = Box::new(TimeTool);
    let list_files: Box<dyn Tool> = Box::new(ListFiles);
    let read_tool: Box<dyn Tool> = Box::new(ReadTool);
    let web_search: Box<dyn Tool> = Box::new(WebSearch);
    let web_fetch: Box<dyn Tool> = Box::new(WebFetch);
    let search_and_replace_tool: Box<dyn Tool> = Box::new(SearchAndReplaceTool);
    let replace_file_tool: Box<dyn Tool> = Box::new(ReplaceFileTool);
    let grep: Box<dyn Tool> = Box::new(GrepTool);
    let find: Box<dyn Tool> = Box::new(FindTool);
    let manpage: Box<dyn Tool> = Box::new(ManPageTool);

    let mut registry: ToolRegistry = ToolRegistry::new();
    registry.register(time_tool);
    registry.register(list_files);
    registry.register(search_and_replace_tool);
    registry.register(read_tool);
    registry.register(web_search);
    registry.register(web_fetch);
    registry.register(manpage);
    registry.register(grep);
    registry.register(find);

    let agent = Agent::new(AgentConfig {
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
        if let Ok(_) = std::io::stdin().read_line(&mut input) {
            if "/close".eq(&input) {
                break;
            }
            agent.run(&input).await;
        }
    }
}
