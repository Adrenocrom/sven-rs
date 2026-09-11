use chrono::Local;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;

#[derive(Deserialize, Debug, JsonSchema)]
struct TimeToolParams;
tool!(TimeTool, TimeToolParams, "get local date time.", execute(_args) {
    let now = Local::now();
    Ok(now.to_string())
});
