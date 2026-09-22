//! Skill management tools backed by `skills/<name>/SKILL.md` (see
//! `skill_spec.md`): add, update, remove, list, search and get.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::skills;

#[derive(Deserialize, Debug, JsonSchema)]
struct AddSkillParams {
    /// Skill name; sanitized to snake_case (e.g. "weather-api-wttr" → "weather_api_wttr")
    name: String,
    /// One-sentence, retrieval-oriented summary; mention key technologies by name
    description: String,
    /// 3-8 lowercase hyphenated keywords (e.g. ["weather", "api", "curl"])
    tags: Vec<String>,
    /// Markdown body of the skill (reference material, examples, caveats)
    content: String,
}

tool!(AddSkillTool, AddSkillParams, "Store new knowledge as a skill in skills/<name>/SKILL.md. Use when you learned something worth remembering for future tasks. The name is sanitized to snake_case; tags to lowercase hyphenated keywords.", execute(args) {
    let skill = skills::add_skill(&args.name, &args.description, &args.tags, &args.content)?;
    Ok(format!(
        "Created skill '{}' at {} with tags [{}].",
        skill.name,
        skills::skill_file(&skill.name).display(),
        skill.tags.join(", ")
    ))
});

#[derive(Deserialize, Debug, JsonSchema)]
struct UpdateSkillParams {
    /// Name of the skill to update (snake_case or kebab-case)
    name: String,
    /// New description (optional)
    description: Option<String>,
    /// New tags, replacing all existing tags (optional)
    tags: Option<Vec<String>>,
    /// New markdown body, replacing the whole body (optional)
    content: Option<String>,
}

tool!(UpdateSkillTool, UpdateSkillParams, "Update an existing skill's description, tags and/or markdown body. The name and creation date are preserved. At least one field must be provided.", execute(args) {
    let skill = skills::update_skill(
        &args.name,
        args.description.as_deref(),
        args.tags.as_deref(),
        args.content.as_deref(),
    )?;
    Ok(format!(
        "Updated skill '{}' at {}.",
        skill.name,
        skills::skill_file(&skill.name).display()
    ))
});

#[derive(Deserialize, Debug, JsonSchema)]
struct RemoveSkillParams {
    /// Name of the skill to remove (snake_case or kebab-case)
    name: String,
}

tool!(RemoveSkillTool, RemoveSkillParams, "Remove a skill and its directory. Use only when the knowledge is wrong or obsolete.", execute(args) {
    Ok(skills::remove_skill(&args.name)?)
});

tool!(ListSkillsTool, "List all stored skills with name, description and tags. Use this to see what knowledge is already available before creating a new skill.", execute() {
    let (skills, errors) = skills::load_all();
    let mut out = String::new();
    if skills.is_empty() {
        out.push_str("No skills stored yet.\n");
    }
    for skill in &skills {
        out.push_str(&format!(
            "- {} — {}\n  tags: [{}]\n",
            skill.name,
            skill.description,
            skill.tags.join(", ")
        ));
    }
    if !errors.is_empty() {
        out.push_str("\nUnreadable skills:\n");
        for e in &errors {
            out.push_str(&format!("- {}\n", e));
        }
    }
    Ok(out)
});

#[derive(Deserialize, Debug, JsonSchema)]
struct SearchSkillsParams {
    /// Keywords to search for (matched against name, description, tags and body)
    query: String,
    /// Maximum number of results (default: 5)
    limit: Option<u64>,
}

tool!(SearchSkillsTool, SearchSkillsParams, "Search stored skills by keywords against name, description, tags and body. Returns the best matches with relevance scores. Use this to find relevant knowledge before answering a task.", execute(args) {
    let limit = args.limit.unwrap_or(5).clamp(1, 50) as usize;
    let hits = skills::search_skills(&args.query, limit);
    if hits.is_empty() {
        return Ok(format!("No skills match {:?}.", args.query));
    }
    let mut out = String::new();
    for hit in &hits {
        out.push_str(&format!(
            "- {} (score {}, matched: {}) — {}\n  tags: [{}]\n",
            hit.name,
            hit.score,
            hit.matched.join(", "),
            hit.description,
            hit.tags.join(", ")
        ));
    }
    Ok(out)
});

#[derive(Deserialize, Debug, JsonSchema)]
struct GetSkillParams {
    /// Name of the skill to retrieve (snake_case or kebab-case)
    name: String,
}

tool!(GetSkillTool, GetSkillParams, "Get the full content of a stored skill (frontmatter and markdown body). Use after SearchSkillsTool to read the actual knowledge.", execute(args) {
    Ok(skills::read_skill_file(&args.name)?)
});
