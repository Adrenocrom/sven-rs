pub mod git_tools;
pub mod compile_tool;
pub mod edit_tool;
pub mod find_tool;
pub mod grep_tool;
pub mod list_files;
pub mod load_attachment_tool;
pub mod manpage_tool;
pub mod mcp_tool;
pub mod notify_tool;
pub mod read_tool;
pub mod skill_tools;
pub mod spawn_agent_tool;
pub mod subprocess;
pub mod time_tool;
pub mod web_fetch;
pub mod web_search;

use crate::sven::tool::Tool;
use crate::sven::tools::compile_tool::{CompileTool, TestTool};
use crate::sven::tools::edit_tool::{ReplaceFileTool, SearchAndReplaceTool};
use crate::sven::tools::find_tool::FindTool;
use crate::sven::tools::git_tools::GitDiffTool;
use crate::sven::tools::grep_tool::GrepTool;
use crate::sven::tools::list_files::ListFiles;
use crate::sven::tools::load_attachment_tool::LoadAttachmentTool;
use crate::sven::tools::manpage_tool::ManPageTool;
use crate::sven::tools::notify_tool::NotifySendTool;
use crate::sven::tools::read_tool::ReadTool;
use crate::sven::tools::skill_tools::{
    AddSkillTool, GetSkillTool, ListSkillsTool, RemoveSkillTool, SearchSkillsTool,
    UpdateSkillTool,
};
use crate::sven::tools::time_tool::TimeTool;
use crate::sven::tools::web_fetch::WebFetch;
use crate::sven::tools::web_search::WebSearch;

/// The built-in tools, boxed for a registry. Called once per agent —
/// the main one in `build_agent`, every sub-agent in
/// `SpawnAgentTool::sub_registry` — so a sub-agent gets exactly the
/// same tool set as its parent (minus the spawn tool itself, which the
/// sub-registry deliberately leaves out). The tools are stateless unit
/// structs, so building the list fresh is cheaper than any sharing
/// machinery.
pub fn builtin_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(TimeTool),
        Box::new(ListFiles),
        Box::new(ReadTool),
        Box::new(WebSearch),
        Box::new(WebFetch),
        Box::new(SearchAndReplaceTool),
        Box::new(ReplaceFileTool),
        Box::new(GrepTool),
        Box::new(FindTool),
        Box::new(ManPageTool),
        Box::new(NotifySendTool),
        Box::new(AddSkillTool),
        Box::new(UpdateSkillTool),
        Box::new(RemoveSkillTool),
        Box::new(ListSkillsTool),
        Box::new(SearchSkillsTool),
        Box::new(GetSkillTool),
        Box::new(CompileTool),
        Box::new(TestTool),
        Box::new(GitDiffTool),
        Box::new(LoadAttachmentTool),
    ]
}
