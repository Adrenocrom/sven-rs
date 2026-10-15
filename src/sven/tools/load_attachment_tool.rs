//! Load a local file as a multimodal attachment — the tool-call
//! counterpart of an `@path` mention in the prompt. The model can pull
//! in an image, audio or video file mid-conversation, so the user does
//! not have to mention every media file up front.
//!
//! Hand-written `Tool` impl instead of the `tool!` macro because it
//! returns attachments, not just text — same pattern as `McpTool`.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::sven::attachment::Attachment;
use crate::sven::security;
use crate::sven::tool::{Tool, ToolFuture, ToolOutput};

#[derive(Deserialize, Debug, JsonSchema)]
struct LoadAttachmentToolParams {
    /// path of the media file to attach (png jpg jpeg gif webp bmp wav mp3 ogg flac m4a mp4 m4v webm mkv mov)
    path: String,
}

pub struct LoadAttachmentTool;

impl Tool for LoadAttachmentTool {
    fn name(&self) -> String {
        "LoadAttachmentTool".to_string()
    }

    fn desc(&self) -> String {
        "Load a local media file as an attachment the model can see or hear \
         (image, audio or video; e.g. png jpg gif webp wav mp3 mp4 webm). \
         Use when the model must perceive a file's content — a screenshot, \
         diagram, recording — rather than read it as text."
            .to_string()
    }

    fn params(&self) -> Option<Value> {
        let mut schema = schemars::schema_for!(LoadAttachmentToolParams);
        schema.remove("$schema");
        Some(serde_json::json!(schema))
    }

    fn execute<'a>(&'a self, params: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: LoadAttachmentToolParams = serde_json::from_value(params)?;
            security::is_inside_cwd(&args.path)?;
            let attachment = Attachment::from_file(&args.path)
                .map_err(|reason| format!("could not attach '{}': {reason}", args.path))?;

            // The confirming text is all the model sees of the payload —
            // the bytes travel in the attachment, never in the text (the
            // agent's truncation must not be able to cut a base64 string).
            let mime = match &attachment {
                Attachment::Image { mime, .. }
                | Attachment::Audio { mime, .. }
                | Attachment::Video { mime, .. } => mime.as_str(),
            };
            let size = std::fs::metadata(&args.path).map(|m| m.len()).unwrap_or(0);
            let text = format!(
                "attached {} '{}' ({}, {:.1} KB)",
                attachment.kind(),
                args.path,
                mime,
                size as f64 / 1024.0,
            );
            Ok(ToolOutput { text, attachments: vec![attachment] })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A file inside the workspace — tests run from the crate root, so
    /// target/ is inside the cwd, which the confinement check requires.
    fn workspace_file(name: &str, bytes: &[u8]) -> String {
        let dir = std::path::Path::new("target")
            .join(format!("load-attachment-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn loads_an_image_as_an_attachment() {
        let path = workspace_file("shot.png", b"png bytes");
        let output = LoadAttachmentTool
            .execute(json!({ "path": path.clone() }))
            .await
            .unwrap();

        assert_eq!(
            output.attachments,
            vec![Attachment::Image {
                data: crate::sven::attachment::base64_encode(b"png bytes"),
                mime: "image/png".to_string(),
            }]
        );
        assert!(output.text.contains("image"), "{}", output.text);
        assert!(output.text.contains("shot.png"), "{}", output.text);
        assert!(output.text.contains("image/png"), "{}", output.text);
    }

    #[tokio::test]
    async fn loads_audio_and_video_by_extension() {
        let audio = workspace_file("clip.wav", b"aud");
        let output = LoadAttachmentTool.execute(json!({ "path": audio })).await.unwrap();
        assert_eq!(output.attachments[0].kind(), "audio");

        let video = workspace_file("clip.mp4", b"vid");
        let output = LoadAttachmentTool.execute(json!({ "path": video })).await.unwrap();
        assert_eq!(output.attachments[0].kind(), "video");
    }

    #[tokio::test]
    async fn reports_unsupported_missing_and_oversized_files() {
        let text_file = workspace_file("notes.txt", b"text");
        let err = LoadAttachmentTool.execute(json!({ "path": text_file })).await.unwrap_err();
        assert!(err.to_string().contains("unsupported file type"), "{err}");

        let err = LoadAttachmentTool
            .execute(json!({ "path": "no-such-file.png" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("file not found"), "{err}");

        let big = workspace_file("big.png", &vec![0u8; 10 * 1024 * 1024 + 1]);
        let err = LoadAttachmentTool.execute(json!({ "path": big })).await.unwrap_err();
        assert!(err.to_string().contains("attachment limit"), "{err}");
    }

    #[tokio::test]
    async fn rejects_paths_outside_the_workspace() {
        let err = LoadAttachmentTool
            .execute(json!({ "path": "/etc/hostname.png" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("outside"), "{err}");
    }
}