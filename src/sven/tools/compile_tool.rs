use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::tools::subprocess;

#[derive(Deserialize, Debug, JsonSchema)]
enum Language {
    Rust
}
#[derive(Deserialize, Debug, JsonSchema)]
struct CompileToolParams {
    /// the language to compile, if rust is selected cargo build will be called
    language: Language
}
fn compile_rust() -> Result<String, Box<dyn std::error::Error>> {
    let mut command = Command::new("cargo");
    command.arg("build");
    Ok(subprocess::run(&mut command)?)
}

tool!(CompileTool, CompileToolParams, "Compile to check for errors.", execute(args) {
    match args.language {
        Language::Rust => return Ok(compile_rust()?),
    }
});