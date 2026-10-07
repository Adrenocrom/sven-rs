use tokio::process::Command;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::sven::macros::tool;
use crate::sven::tools::subprocess;

#[derive(Deserialize, Debug, JsonSchema)]
enum Language {
    Rust,
    UefiRust,
    Java,
    Dotnet,
    Python
}
#[derive(Deserialize, Debug, JsonSchema)]
struct CompileToolParams {
    /// the language to compile, if rust is selected cargo build will be called
    language: Language
}
async fn compile_rust() -> Result<String, Box<dyn std::error::Error>> {
    let mut command = Command::new("cargo");
    command.arg("build");
    Ok(subprocess::run(&mut command).await?)
}

async fn compile_uefi_rust() -> Result<String, Box<dyn std::error::Error>> {
    let mut command = Command::new("cargo");
    command.arg("build");
    command.arg("--release");
    command.arg("--target=x86_64-unknown-uefi");
    Ok(subprocess::run(&mut command).await?)
}

async fn compile_java() -> Result<String, Box<dyn std::error::Error>> {
    let mut command = Command::new("mvn");
    command.arg("clean");
    command.arg("install");
    Ok(subprocess::run(&mut command).await?)
}

async fn compile_dotnet() -> Result<String, Box<dyn std::error::Error>> {
    let mut command = Command::new("dotnet");
    command.arg("build");
    Ok(subprocess::run(&mut command).await?)
}

async fn compile_python() -> Result<String, Box<dyn std::error::Error>> {
    let mut command = Command::new("python");
    command.arg("-m");
    command.arg("compileall");
    command.arg("-q");
    command.arg(".");
    Ok(subprocess::run(&mut command).await?)
}

tool!(CompileTool, CompileToolParams, "Compile to check for errors.", execute(args) {
    match args.language {
        Language::Rust => return Ok(compile_rust().await?),
        Language::UefiRust => return Ok(compile_uefi_rust().await?),
        Language::Java => return Ok(compile_java().await?),
        Language::Dotnet => return Ok(compile_dotnet().await?),
        Language::Python => return Ok(compile_python().await?)
    }
});
