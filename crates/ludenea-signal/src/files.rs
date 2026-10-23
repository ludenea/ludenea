//! Serving the files of a folder read-only, without ever leading out of it.

use std::path::Path;

use axum::body::{Body, Bytes};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::io::AsyncReadExt;

/// The size of the pieces a file is sent in: a response holds one at a time, whatever the
/// file's size.
const CHUNK: usize = 64 * 1024;

/// The file at `path` with its content type and length, streamed, or 404 when it cannot be
/// opened.
pub async fn serve_file(path: &Path) -> Response {
    let Ok(file) = tokio::fs::File::open(path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(metadata) = file.metadata().await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !metadata.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let chunks = futures_util::stream::try_unfold(file, |mut file| async move {
        let mut chunk = vec![0; CHUNK];
        let read = file.read(&mut chunk).await?;
        if read == 0 {
            return Ok::<_, std::io::Error>(None);
        }
        chunk.truncate(read);
        Ok(Some((Bytes::from(chunk), file)))
    });
    (
        [
            (header::CONTENT_TYPE, content_type(path).to_string()),
            (header::CONTENT_LENGTH, metadata.len().to_string()),
        ],
        Body::from_stream(chunks),
    )
        .into_response()
}

/// The path, relative to a served folder, that the URL path `url_path` names: its segments
/// joined, `index.html` for the root. `None` when a segment could lead out of the folder.
pub fn relative_path(url_path: &str) -> Option<String> {
    let segments: Vec<&str> = url_path.split('/').filter(|s| !s.is_empty()).collect();
    if !segments.iter().all(|segment| plain_name(segment)) {
        return None;
    }
    Some(if segments.is_empty() {
        "index.html".to_string()
    } else {
        segments.join("/")
    })
}

/// A single file or directory name: nothing that could lead out of the directory it is
/// looked up in, nor a Windows device name, which opens the device wherever it is.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && !device_name(name)
}

/// Whether Windows reads `name` as a device (`CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9`
/// and `COM¹` to `COM³`, the same for `LPT`): in any case, with any extension, and with
/// trailing spaces before it.
fn device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let stem = stem.to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    };
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
}

/// The content type of a served file, from its extension.
pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_path_maps_inside_the_folder_or_not_at_all() {
        assert_eq!(relative_path("/").as_deref(), Some("index.html"));
        assert_eq!(relative_path("").as_deref(), Some("index.html"));
        assert_eq!(relative_path("/a//b.js").as_deref(), Some("a/b.js"));
        for path in ["/../x", "/a/./b", "/a\\b", "/c:", "/a\0"] {
            assert_eq!(relative_path(path), None, "{path:?}");
        }
    }

    #[test]
    fn windows_device_names_are_not_plain() {
        for name in [
            "CON",
            "con",
            "PRN",
            "Aux",
            "nul",
            "COM1",
            "com9",
            "LPT1",
            "lpt9",
            "NUL.txt",
            "con.tar.gz",
            "AUX ",
            "COM1 .js",
            "COM¹",
            "com².txt",
            "COM³",
            "LPT¹",
            "lpt²",
            "LPT³.js",
        ] {
            assert!(!plain_name(name), "{name:?}");
        }
        for name in [
            "CONSOLE", "icon.png", "COM", "COM10", "LPT0x", "null.js", "aux_data", "COM⁴",
        ] {
            assert!(plain_name(name), "{name:?}");
        }
    }
}
