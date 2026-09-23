use std::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;


#[derive(Deserialize, Debug, JsonSchema)]
enum Language {
    Rust
}
#[derive(Deserialize, Debug, JsonSchema)]
struct CompileToolParams {
    ///  the language to compile, if rust is selected cargo build will be called
    lanuguage: Language
}
fn compile_rust() -> Result<String, Box<dyn std::error::Error>> {
    let mut command = Command::new("cargo");
    command.arg("build");
    let output = command.output()?;
    let out = String::from_utf8(output.stderr)?;
    Ok(out.to_string())
}

tool!(CompileTool, CompileToolParams, "Compile to check for errors.", execute(args) {
    match args.lanuguage {
        Language::Rust => return Ok(compile_rust()?),
    }
});
