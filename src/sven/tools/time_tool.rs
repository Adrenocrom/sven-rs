use chrono::Local;

use crate::sven::macros::tool;

tool!(TimeTool, "get local date time.", execute() {
    let now = Local::now();
    Ok(now.to_string())
});
