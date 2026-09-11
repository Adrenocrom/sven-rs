use std::io::BufRead;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::security;

#[derive(Deserialize, Debug, JsonSchema)]
struct ReadToolParams {
    /// file path
    path: String,
    /// number of lines to read (default: whole file)
    num_lines:  Option<u64>,
    /// offset to start reading from (default: 0)
    offset:  Option<u64>,
}
tool!(ReadTool, ReadToolParams, "Read a file from a given path, with optional line offset and count.", execute(args) {
    security::is_inside_cwd(&args.path)?;
    let offset = match args.offset {
        Some(n) if n > 0 => n as usize,
        _ => 0,
    };

    let num_lines = match args.num_lines {
        Some(n) if n > 0 => Some((n as usize) + offset),
        _ => None,
    };

    let file = std::fs::File::open(args.path)?;
    let reader = std::io::BufReader::new(&file);
    let mut result = String::new();

    for (idx, line) in reader.lines().enumerate() {
        if idx < offset {
            continue;
        }
        if let Some(num_lines) = num_lines {
            if idx >= num_lines {
                break;
            }
        }

        let line = line?;
        result.push_str(&line);
        result.push('\n');
    }

    Ok(result)
});
