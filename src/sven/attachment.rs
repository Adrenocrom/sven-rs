//! Multimodal attachments: images, audio and video folded into chat
//! messages. `@<path>` mentions in a prompt are loaded from disk (see
//! `expand`); MCP servers deliver them as base64 content items. The
//! `Attachment` enum is backend-neutral — `Backend::apply_attachments`
//! turns it into the wire format of the configured server, and
//! `filter_supported` drops what the server cannot carry.

use std::collections::BTreeSet;
use std::path::Path;

use crate::sven::backend::Backend;
use crate::sven::security;

/// Maximum size of one file loaded via an `@` mention, in raw bytes
/// (before base64). Base64 inflates the payload by a third and the whole
/// history is re-sent on every chat round, so one huge file would
/// dominate every request.
pub const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;

/// File extensions `@` mentions can attach. The extension decides the
/// modality; the actual bytes are never inspected.
const SUPPORTED_EXTENSIONS: &str =
    "png jpg jpeg gif webp bmp wav mp3 ogg flac m4a mp4 m4v webm mkv mov";

/// One binary attachment. `data` is base64; `mime` is the full MIME type
/// (`image/png`, `audio/wav`, …).
#[derive(Clone, Debug, PartialEq)]
pub enum Attachment {
    Image { data: String, mime: String },
    Audio { data: String, mime: String },
    Video { data: String, mime: String },
}

impl Attachment {
    /// The modality, as named in warnings ("image", "audio", "video").
    pub fn kind(&self) -> &'static str {
        match self {
            Attachment::Image { .. } => "image",
            Attachment::Audio { .. } => "audio",
            Attachment::Video { .. } => "video",
        }
    }

    /// Load a file as an attachment: MIME type by extension, size
    /// capped, base64-encoded. Path confinement is the caller's job —
    /// `expand` checks it; MCP attachments arrive as data, not paths.
    pub fn from_file(path: &str) -> Result<Attachment, String> {
        let mime = mime_for(path)
            .ok_or_else(|| format!("unsupported file type (supported: {SUPPORTED_EXTENSIONS})"))?;
        let metadata = std::fs::metadata(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "file not found".to_string(),
            _ => e.to_string(),
        })?;
        if !metadata.is_file() {
            return Err("not a regular file".to_string());
        }
        if metadata.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "{:.1} MB exceeds the {} MB attachment limit",
                metadata.len() as f64 / (1024.0 * 1024.0),
                MAX_ATTACHMENT_BYTES / (1024 * 1024)
            ));
        }
        let data = base64_encode(&std::fs::read(path).map_err(|e| e.to_string())?);
        Ok(if mime.starts_with("image/") {
            Attachment::Image { data, mime: mime.to_string() }
        } else if mime.starts_with("audio/") {
            Attachment::Audio { data, mime: mime.to_string() }
        } else {
            Attachment::Video { data, mime: mime.to_string() }
        })
    }
}

/// The MIME type of an attachable file, by extension — `None` for
/// everything else (including files without an extension).
fn mime_for(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        _ => return None,
    })
}

/// The `@<path>` mentions in a prompt: a whitespace-delimited token
/// starting with `@` whose path has a supported extension. Trailing
/// sentence punctuation is trimmed ("look at @ui.png."), and an `@`
/// inside a word ("user@example.com") is not a mention.
fn parse_mentions(prompt: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    for token in prompt.split_whitespace() {
        let Some(path) = token.strip_prefix('@') else {
            continue;
        };
        let path = path.trim_matches(|c| {
            matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '(' | ')' | '"' | '\'')
        });
        if !path.is_empty() && mime_for(path).is_some() {
            mentions.push(path.to_string());
        }
    }
    mentions
}

/// Load every `@<path>` mention in `prompt` that points at an
/// attachable file inside the workspace. Returns the attachments the
/// backend can carry plus one warning per mention that failed (missing
/// file, too large, outside the workspace, unsupported modality). The
/// prompt text is unchanged — the caller appends the warnings as
/// bracketed notes so the model knows why a mentioned file is missing.
pub fn expand(prompt: &str, backend: &Backend) -> (Vec<Attachment>, Vec<String>) {
    let mut attachments = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    for path in parse_mentions(prompt) {
        // the same file mentioned twice is attached once
        if !seen.insert(path.clone()) {
            continue;
        }
        let attachment = match security::is_inside_cwd(&path) {
            Ok(()) => Attachment::from_file(&path),
            Err(e) => Err(e.to_string()),
        };
        match attachment {
            Ok(attachment) => attachments.push(attachment),
            Err(reason) => warnings.push(format!("could not attach '{path}': {reason}")),
        }
    }
    let (supported, dropped) = filter_supported(backend, attachments);
    warnings.extend(dropped);
    (supported, warnings)
}

/// Split off the attachments the backend cannot carry, with one note
/// per dropped modality. Nothing is dropped silently — the notes are
/// meant for the model ("you will not hear the audio") and the terminal.
pub fn filter_supported(
    backend: &Backend,
    attachments: Vec<Attachment>,
) -> (Vec<Attachment>, Vec<String>) {
    let mut kept = Vec::new();
    let mut dropped: Vec<&'static str> = Vec::new();
    for attachment in attachments {
        if backend.supports_attachment(&attachment) {
            kept.push(attachment);
        } else if !dropped.contains(&attachment.kind()) {
            dropped.push(attachment.kind());
        }
    }
    let notes = dropped
        .into_iter()
        .map(|kind| {
            format!(
                "dropped {kind} attachment: backend {} does not support {kind}",
                backend.to_string()
            )
        })
        .collect();
    (kept, notes)
}

/// Standard base64 (RFC 4648, with padding) — the encoding every chat API
/// expects for binary content. Hand-rolled like the MCP client: no
/// dependency for something this small.
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        out.push(BASE64_ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(BASE64_ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // the PNG magic number, as it appears in Ollama's API examples
        assert_eq!(
            base64_encode(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
            "iVBORw0KGgo"
        );
    }

    #[test]
    fn mime_types_follow_the_extension() {
        assert_eq!(mime_for("a.png"), Some("image/png"));
        assert_eq!(mime_for("a.JPG"), Some("image/jpeg"));
        assert_eq!(mime_for("a.mp3"), Some("audio/mpeg"));
        assert_eq!(mime_for("dir/a.mp4"), Some("video/mp4"));
        assert_eq!(mime_for("archive.tar.gz"), None);
        assert_eq!(mime_for("noext"), None);
    }

    #[test]
    fn mentions_are_at_paths_with_supported_extensions() {
        let mentions = parse_mentions("look at @ui.png and @docs/diagram.svg, plus @clip.mp4!");
        assert_eq!(mentions, ["ui.png", "clip.mp4"]);
        // @ inside a word is not a mention; unknown extensions are ignored
        assert!(parse_mentions("mail user@example.com @notes.txt").is_empty());
        assert_eq!(parse_mentions("@x.png").len(), 1);
    }

    /// A file inside the workspace — tests run from the crate root, so
    /// target/ is inside the cwd, which the confinement check in
    /// `expand` requires.
    fn workspace_file(name: &str, bytes: &[u8]) -> String {
        let dir = Path::new("target").join(format!("attachment-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn loads_files_as_attachments() {
        let path = workspace_file("tiny.png", b"not really a png");
        let attachment = Attachment::from_file(&path).unwrap();
        assert_eq!(
            attachment,
            Attachment::Image {
                data: base64_encode(b"not really a png"),
                mime: "image/png".to_string(),
            }
        );
    }

    #[test]
    fn rejects_oversized_files() {
        let path = workspace_file("big.png", &vec![0u8; MAX_ATTACHMENT_BYTES as usize + 1]);
        let error = Attachment::from_file(&path).unwrap_err();
        assert!(error.contains("attachment limit"), "{}", error);
    }

    #[test]
    fn expand_reports_what_could_not_be_attached() {
        let image = workspace_file("shot.png", b"img");
        let audio = workspace_file("clip.wav", b"aud");

        // Ollama carries images but not audio
        let (attachments, warnings) = expand(
            &format!("look at @{image} and @does-not-exist.png and @{audio}"),
            &Backend::Ollama,
        );
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind(), "image");
        assert_eq!(warnings.len(), 2, "{:?}", warnings);
        assert!(
            warnings.iter().any(|w| w.contains("file not found")),
            "{:?}",
            warnings
        );
        assert!(
            warnings.iter().any(|w| w.contains("does not support audio")),
            "{:?}",
            warnings
        );

        // vLLM carries both; the same file mentioned twice attaches once
        let (attachments, warnings) =
            expand(&format!("look at @{image} and @{audio} and @{image}"), &Backend::Vllm);
        assert_eq!(attachments.len(), 2);
        assert!(warnings.is_empty(), "{:?}", warnings);
    }
}