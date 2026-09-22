//! Skill storage, following `skill_spec.md`.
//!
//! Layout: `skills/<kebab-case-name>/SKILL.md` — YAML frontmatter
//! (name, description, tags, created_at) followed by a markdown body.
//!
//! The frontmatter parser is hand-rolled and intentionally minimal: it
//! supports exactly what the spec's flat schema needs — `key: value`
//! scalars, single/double-quoted scalars, block lists (`- item`) and
//! folded multi-line scalars (indented continuation lines, joined with
//! spaces). Not supported: nested maps, anchors, flow style (`[a, b]`),
//! multi-document streams.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::Local;

pub const SKILLS_DIR: &str = "/home/josef/.config/sven/skills";
pub const SKILL_FILE: &str = "SKILL.md";

// ---------------------------------------------------------------------------
// Minimal YAML parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum YamlValue {
    Scalar(String),
    List(Vec<String>),
}

/// Parse a flat YAML document into a key → value map.
pub fn parse_yaml(input: &str) -> Result<HashMap<String, YamlValue>, String> {
    let mut map: HashMap<String, YamlValue> = HashMap::new();
    // key that the next list item / folded continuation line belongs to
    let mut current: Option<String> = None;

    for (no, raw) in input.lines().enumerate() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        let line_no = no + 1;
        let is_list_item =
            trimmed.starts_with('-') && (trimmed.len() == 1 || trimmed.as_bytes()[1] == b' ');
        let cur_is_list = matches!(
            current.as_deref().and_then(|k| map.get(k)),
            Some(YamlValue::List(_))
        );
        let cur_is_scalar = matches!(
            current.as_deref().and_then(|k| map.get(k)),
            Some(YamlValue::Scalar(_))
        );

        // `- item` under a list key
        if is_list_item && cur_is_list {
            if let Some(YamlValue::List(items)) = current.as_deref().and_then(|k| map.get_mut(k)) {
                items.push(unquote(trimmed[1..].trim()));
            }
            continue;
        }

        // folded scalar: indented continuation of the previous key's value
        if indent > 0 && cur_is_scalar {
            if let Some(YamlValue::Scalar(value)) = current.as_deref().and_then(|k| map.get_mut(k))
            {
                value.push(' ');
                value.push_str(trimmed);
            }
            continue;
        }

        if is_list_item {
            return Err(format!("line {}: list item outside of a list", line_no));
        }
        if indent > 0 {
            return Err(format!("line {}: unexpected indented line", line_no));
        }

        // top-level `key: value`
        match split_key(trimmed) {
            Some((key, value)) => {
                let key = key.trim().to_string();
                let value = value.trim();
                if value.is_empty() {
                    // bare `key:` — a (possibly empty) list, as written for `tags:`
                    map.insert(key.clone(), YamlValue::List(Vec::new()));
                } else {
                    map.insert(key.clone(), YamlValue::Scalar(unquote(value)));
                }
                current = Some(key);
            }
            None => return Err(format!("line {}: cannot parse {:?}", line_no, raw)),
        }
    }
    Ok(map)
}

/// Split `key: value` at the first colon followed by a space or end of line.
fn split_key(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' && (i + 1 == bytes.len() || bytes[i + 1] == b' ') {
            return Some((&line[..i], &line[i + 1..]));
        }
    }
    None
}

/// Strip surrounding quotes (unescaping `''` in single-quoted scalars) and
/// strip ` #` comments from unquoted values.
fn unquote(raw: &str) -> String {
    let s = raw.trim();
    if s.len() >= 2 {
        let first = s.as_bytes()[0];
        let last = s.as_bytes()[s.len() - 1];
        if first == b'\'' && last == b'\'' {
            return s[1..s.len() - 1].replace("''", "'");
        }
        if first == b'"' && last == b'"' {
            return s[1..s.len() - 1].to_string();
        }
    }
    match s.find(" #") {
        Some(i) => s[..i].trim_end().to_string(),
        None => s.to_string(),
    }
}

/// Split a SKILL.md file into (frontmatter, body) at the first two `---`
/// lines, per the spec's implementation notes.
pub fn split_frontmatter(content: &str) -> Result<(String, String), String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return Err("file must start with a '---' line".to_string());
    }
    let close = lines[1..]
        .iter()
        .position(|l| l.trim() == "---")
        .map(|i| i + 1)
        .ok_or_else(|| "missing closing '---' line".to_string())?;
    let frontmatter = lines[1..close].join("\n");
    let body = lines[close + 1..]
        .join("\n")
        .trim_start_matches('\n')
        .to_string();
    Ok((frontmatter, body))
}

// ---------------------------------------------------------------------------
// Skill model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Skill {
    /// snake_case identifier (directory name with `-` → `_`)
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    /// ISO 8601 timestamp with timezone offset; set once at creation
    pub created_at: String,
    /// markdown body
    pub body: String,
}

/// Quote a scalar for writing: single-quote it if it could be misparsed
/// (colon, `#`, quote char, leading YAML indicator, surrounding space).
fn yaml_quote(s: &str) -> String {
    let needs = s.is_empty()
        || s.trim() != s
        || s.contains(':')
        || s.contains('#')
        || s.contains('\'')
        || s.chars().next().is_some_and(|c| "-&*!%@`\"{[|>,".contains(c));
    if needs {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        s.to_string()
    }
}

impl Skill {
    /// Serialize to SKILL.md format (frontmatter + body). Newlines in the
    /// description are folded to spaces, matching the parser.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str("name: ");
        out.push_str(&self.name);
        out.push('\n');
        out.push_str("description: ");
        out.push_str(&yaml_quote(&self.description.replace('\n', " ")));
        out.push('\n');
        out.push_str("tags:\n");
        for tag in &self.tags {
            out.push_str("- ");
            out.push_str(&yaml_quote(tag));
            out.push('\n');
        }
        out.push_str("created_at: '");
        out.push_str(&self.created_at);
        out.push_str("'\n---\n\n");
        out.push_str(&self.body);
        out.push('\n');
        out
    }
}

// ---------------------------------------------------------------------------
// Name sanitization (spec §8)
// ---------------------------------------------------------------------------

/// Lowercase alphanumerics; runs of other characters collapse into `sep`;
/// no leading/trailing separator.
fn sanitize(name: &str, sep: char) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push(sep);
            }
            pending_sep = false;
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() {
            pending_sep = true;
        }
    }
    out
}

/// snake_case identifier for the frontmatter `name`.
pub fn to_snake(name: &str) -> String {
    sanitize(name, '_')
}

/// kebab-case directory name.
pub fn to_kebab(name: &str) -> String {
    sanitize(name, '-')
}

fn is_snake(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

pub fn skills_root() -> PathBuf {
    PathBuf::from(SKILLS_DIR)
}

/// Directory of a skill; accepts snake_case or kebab-case input.
pub fn skill_dir(name: &str) -> PathBuf {
    skills_root().join(to_kebab(name))
}

pub fn skill_file(name: &str) -> PathBuf {
    skill_dir(name).join(SKILL_FILE)
}

// ---------------------------------------------------------------------------
// Parsing / loading
// ---------------------------------------------------------------------------

/// Parse SKILL.md content. `dir` is the skill's directory name
/// (kebab-case); the frontmatter `name` must match it (spec §7).
pub fn parse_skill(content: &str, dir: &str) -> Result<Skill, String> {
    let (frontmatter, body) = split_frontmatter(content)?;
    let map = parse_yaml(&frontmatter)?;

    let scalar = |key: &str| -> Option<String> {
        match map.get(key) {
            Some(YamlValue::Scalar(s)) => Some(s.clone()),
            _ => None,
        }
    };

    let name = scalar("name").ok_or("frontmatter: missing 'name'")?;
    let description = scalar("description").ok_or("frontmatter: missing 'description'")?;
    let created_at = scalar("created_at").ok_or("frontmatter: missing 'created_at'")?;
    let tags = match map.get("tags") {
        Some(YamlValue::List(tags)) => tags.clone(),
        Some(YamlValue::Scalar(_)) => return Err("frontmatter: 'tags' must be a list".to_string()),
        None => return Err("frontmatter: missing 'tags'".to_string()),
    };

    if !is_snake(&name) {
        return Err(format!(
            "frontmatter: 'name' must be snake_case, got {:?}",
            name
        ));
    }
    let expected = dir.replace('-', "_");
    if name != expected {
        return Err(format!(
            "frontmatter: 'name' ({}) does not match directory name ({})",
            name, dir
        ));
    }
    if description.trim().is_empty() {
        return Err("frontmatter: 'description' is empty".to_string());
    }

    Ok(Skill {
        name,
        description,
        tags,
        created_at,
        body,
    })
}

/// Raw SKILL.md content of one skill (frontmatter + body).
///
/// No `is_inside_cwd` check: the path is built from the fixed `SKILLS_DIR`
/// plus a name sanitized to `[a-z0-9-]`, so it cannot escape the store. The
/// cwd check could never pass for a global config dir and made GetSkill and
/// RemoveSkill fail unconditionally.
pub fn read_skill_file(name: &str) -> Result<String, String> {
    let kebab = to_kebab(name);
    if kebab.is_empty() {
        return Err(format!("invalid skill name {:?}", name));
    }
    let path = skills_root().join(&kebab).join(SKILL_FILE);
    fs::read_to_string(&path).map_err(|e| format!("cannot read skill '{}': {}", name, e))
}

/// Load and validate one skill by name (snake_case or kebab-case).
pub fn load_skill(name: &str) -> Result<Skill, String> {
    let kebab = to_kebab(name);
    if kebab.is_empty() {
        return Err(format!("invalid skill name {:?}", name));
    }
    let content = read_skill_file(name)?;
    parse_skill(&content, &kebab)
}

/// Load every valid skill. Broken skills are reported as errors instead of
/// aborting the whole listing.
pub fn load_all() -> (Vec<Skill>, Vec<String>) {
    let mut skills = Vec::new();
    let mut errors = Vec::new();

    let Ok(entries) = fs::read_dir(skills_root()) else {
        return (skills, errors); // no skills/ directory yet
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let file = dir.join(SKILL_FILE);
        if !file.is_file() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        match fs::read_to_string(&file).map_err(|e| e.to_string()) {
            Ok(content) => match parse_skill(&content, &dir_name) {
                Ok(skill) => skills.push(skill),
                Err(e) => errors.push(format!("{}: {}", dir_name, e)),
            },
            Err(e) => errors.push(format!("{}: {}", dir_name, e)),
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    (skills, errors)
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// Sanitize tags to lowercase hyphenated keywords, deduplicated, and
/// enforce the spec's 3–8 range.
fn sanitize_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = to_kebab(tag);
        if !tag.is_empty() && !out.contains(&tag) {
            out.push(tag);
        }
    }
    if !(3..=8).contains(&out.len()) {
        return Err(format!(
            "expected 3-8 tags after sanitization, got {}",
            out.len()
        ));
    }
    Ok(out)
}

/// Create a new skill. The name is sanitized to a snake_case identifier /
/// kebab-case directory; fails on duplicates.
pub fn add_skill(
    name: &str,
    description: &str,
    tags: &[String],
    body: &str,
) -> Result<Skill, String> {
    let name = to_snake(name);
    if name.is_empty() {
        return Err("skill name is empty after sanitization".to_string());
    }
    let description = description.trim();
    if description.is_empty() {
        return Err("description must not be empty".to_string());
    }
    let tags = sanitize_tags(tags)?;

    let (existing, _) = load_all();
    if existing.iter().any(|s| s.name == name) {
        return Err(format!(
            "skill '{}' already exists — use the update tool",
            name
        ));
    }
    let dir = skill_dir(&name);
    if dir.exists() {
        return Err(format!("directory {} already exists", dir.display()));
    }

    let skill = Skill {
        name,
        description: description.to_string(),
        tags,
        created_at: Local::now().to_rfc3339(),
        body: body.trim().to_string(),
    };
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    let file = skill_file(&skill.name);
    fs::write(&file, skill.to_markdown())
        .map_err(|e| format!("cannot write {}: {}", file.display(), e))?;
    Ok(skill)
}

/// Update fields of an existing skill. `created_at` and the name are
/// preserved (the directory name is the identifier).
pub fn update_skill(
    name: &str,
    description: Option<&str>,
    tags: Option<&[String]>,
    body: Option<&str>,
) -> Result<Skill, String> {
    if description.is_none() && tags.is_none() && body.is_none() {
        return Err("nothing to update: provide description, tags and/or content".to_string());
    }
    let mut skill = load_skill(name)?;
    if let Some(description) = description {
        let description = description.trim();
        if description.is_empty() {
            return Err("description must not be empty".to_string());
        }
        skill.description = description.to_string();
    }
    if let Some(tags) = tags {
        skill.tags = sanitize_tags(tags)?;
    }
    if let Some(body) = body {
        skill.body = body.trim().to_string();
    }
    let file = skill_file(&skill.name);
    fs::write(&file, skill.to_markdown())
        .map_err(|e| format!("cannot write {}: {}", file.display(), e))?;
    Ok(skill)
}

/// Remove a skill and its directory.
pub fn remove_skill(name: &str) -> Result<String, String> {
    let kebab = to_kebab(name);
    if kebab.is_empty() {
        return Err(format!("invalid skill name {:?}", name));
    }
    let dir = skills_root().join(&kebab);
    if !dir.join(SKILL_FILE).is_file() {
        return Err(format!("no skill '{}' at {}", name, dir.display()));
    }
    // No `is_inside_cwd` check: `kebab` is sanitized to `[a-z0-9-]`, so the
    // path cannot escape the skills store (see `read_skill_file`).
    fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove {}: {}", dir.display(), e))?;
    Ok(format!("removed skill '{}' ({})", kebab, dir.display()))
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

pub struct SearchHit {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub score: u32,
    /// which fields matched: subset of name, description, tags, body
    pub matched: Vec<String>,
}

/// Keyword search over all skills. The query is split on
/// non-alphanumerics; tokens are matched as substrings against tags
/// (weight 4), name (3), description (2) and body (1).
pub fn search_skills(query: &str, limit: usize) -> Vec<SearchHit> {
    let tokens: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let (skills, _) = load_all();

    let mut hits: Vec<SearchHit> = skills
        .into_iter()
        .filter_map(|skill| {
            let name = skill.name.replace('_', "-");
            let description = skill.description.to_lowercase();
            let body = skill.body.to_lowercase();
            let mut score = 0;
            let mut matched: Vec<&str> = Vec::new();
            for token in &tokens {
                if skill.tags.iter().any(|t| t.contains(token.as_str())) {
                    score += 4;
                    if !matched.contains(&"tags") {
                        matched.push("tags");
                    }
                }
                if name.contains(token.as_str()) {
                    score += 3;
                    if !matched.contains(&"name") {
                        matched.push("name");
                    }
                }
                if description.contains(token.as_str()) {
                    score += 2;
                    if !matched.contains(&"description") {
                        matched.push("description");
                    }
                }
                if body.contains(token.as_str()) {
                    score += 1;
                    if !matched.contains(&"body") {
                        matched.push("body");
                    }
                }
            }
            if score == 0 {
                return None;
            }
            Some(SearchHit {
                name: skill.name,
                description: skill.description,
                tags: skill.tags,
                score,
                matched: matched.into_iter().map(str::to_string).collect(),
            })
        })
        .collect();

    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    hits.truncate(limit);
    hits
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"---
name: weather_api_wttr
description: Free weather API using wttr.in — no API key required, supports JSON output
  with current conditions and 3-day forecasts.
tags:
- weather
- api
- wttr
created_at: '2026-07-12T18:59:41.009310+00:00'
---

# Weather API: wttr.in

**wttr.in** is free.
"#;

    #[test]
    fn parses_spec_example() {
        let skill = parse_skill(EXAMPLE, "weather-api-wttr").unwrap();
        assert_eq!(skill.name, "weather_api_wttr");
        assert_eq!(
            skill.description,
            "Free weather API using wttr.in — no API key required, supports JSON output with current conditions and 3-day forecasts."
        );
        assert_eq!(skill.tags, vec!["weather", "api", "wttr"]);
        assert_eq!(skill.created_at, "2026-07-12T18:59:41.009310+00:00");
        assert!(skill.body.starts_with("# Weather API: wttr.in"));
    }

    #[test]
    fn round_trips_through_markdown() {
        let skill = Skill {
            name: "docs_rs_lookup".to_string(),
            description: "How to look up Rust docs: docs.rs and rustdoc".to_string(),
            tags: vec!["rust".to_string(), "docs".to_string(), "rustdoc".to_string()],
            created_at: "2026-07-12T18:59:41.009310+00:00".to_string(),
            body: "## Overview\n\nUse `docs.rs`.".to_string(),
        };
        let parsed = parse_skill(&skill.to_markdown(), "docs-rs-lookup").unwrap();
        assert_eq!(parsed.name, skill.name);
        assert_eq!(parsed.description, skill.description);
        assert_eq!(parsed.tags, skill.tags);
        assert_eq!(parsed.created_at, skill.created_at);
        assert_eq!(parsed.body, skill.body);
    }

    #[test]
    fn rejects_name_directory_mismatch() {
        let err = parse_skill(EXAMPLE, "weather-api").unwrap_err();
        assert!(err.contains("does not match"));
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(to_snake("Weather API (wttr)"), "weather_api_wttr");
        assert_eq!(to_kebab("weather_api_wttr"), "weather-api-wttr");
        assert_eq!(to_snake("--"), "");
    }

    #[test]
    fn yaml_quoting_round_trip() {
        for value in ["plain", "with: colon", "it's quoted", "has # hash", ""] {
            let quoted = yaml_quote(value);
            let map = parse_yaml(&format!("k: {}", quoted)).unwrap();
            assert_eq!(
                map.get("k"),
                Some(&YamlValue::Scalar(value.to_string())),
                "round trip failed for {:?} (written as {:?})",
                value,
                quoted
            );
        }
    }

    #[test]
    fn parses_quoted_scalars_and_comments() {
        let map = parse_yaml("a: 'x: y'\nb: \"z\"\nc: plain # comment\n").unwrap();
        assert_eq!(map.get("a"), Some(&YamlValue::Scalar("x: y".to_string())));
        assert_eq!(map.get("b"), Some(&YamlValue::Scalar("z".to_string())));
        assert_eq!(map.get("c"), Some(&YamlValue::Scalar("plain".to_string())));
    }

    #[test]
    fn frontmatter_requires_delimiters() {
        assert!(split_frontmatter("no frontmatter").is_err());
        assert!(split_frontmatter("---\nname: x\n").is_err()); // no closing ---
    }
}
