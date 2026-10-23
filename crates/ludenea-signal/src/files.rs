//! Serving the files of a folder read-only, without ever leading out of it.

use std::path::Path;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

/// The file at `path` with its content type, or 404 when it cannot be read.
pub async fn serve_file(path: &Path) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type(path))], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
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
/// looked up in.
pub fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
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
}
