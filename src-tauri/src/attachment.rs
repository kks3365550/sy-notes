use std::path::{Path, PathBuf};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Saves a base64-encoded attachment into `<base_data_dir>/attachments/<uuid>.<ext>`.
/// Enforces directory traversal protection and UUID filename generation.
/// Name of the custom URI scheme that serves attachment files to the webview.
///
/// Documents keep storing the stable relative URL `attachments/<file>`; the
/// frontend maps it to `attachment://localhost/<file>` (or
/// `http://attachment.localhost/<file>` on Windows/Android) for display only.
pub const URI_SCHEME: &str = "attachment";

/// Prefix used for attachment URLs stored in document JSON.
pub const STORED_URL_PREFIX: &str = "attachments/";

/// File extensions that `save_attachment_file` may produce and the URI scheme may serve.
const ALLOWED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "pdf", "txt"];

/// Returns `<base_data_dir>/attachments`.
pub fn attachments_dir(base_data_dir: &Path) -> PathBuf {
    base_data_dir.join("attachments")
}

/// Why an attachment request could not be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// The request path is not a single plain file name (traversal, separators, bad chars, bad ext).
    Invalid,
    /// The name is valid but no regular file directly inside the attachments dir matches it.
    NotFound,
}

/// Returns the MIME type for an allowed attachment extension.
pub fn mime_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

/// Validates that `name` is a bare attachment file name: ASCII letters, digits, `-`, `_`
/// and a single `.` before an allowed extension. Anything else (`/`, `\`, `..`, `%`,
/// `:`, NUL, empty, leading dot, unknown extension) is rejected. Returns the lowercase
/// extension on success.
pub fn validate_attachment_name(name: &str) -> Result<String, ResolveError> {
    if name.is_empty() || name.len() > 255 {
        return Err(ResolveError::Invalid);
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err(ResolveError::Invalid);
    }
    let (stem, ext) = name.rsplit_once('.').ok_or(ResolveError::Invalid)?;
    if stem.is_empty() || stem.contains('.') {
        return Err(ResolveError::Invalid);
    }
    let ext = ext.to_ascii_lowercase();
    if !ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(ResolveError::Invalid);
    }
    Ok(ext)
}

/// Extracts the requested file name from an attachment URI path.
///
/// Accepts `/<file>` (what `convertFileSrc(file, "attachment")` produces) and, for
/// robustness, `/attachments/<file>`. Query strings/fragments must already be stripped.
pub fn file_name_from_uri_path(path: &str) -> Result<&str, ResolveError> {
    let p = path.strip_prefix('/').unwrap_or(path);
    let p = p.strip_prefix(STORED_URL_PREFIX).unwrap_or(p);
    validate_attachment_name(p)?;
    Ok(p)
}

/// Resolves a request path to a file that lives *directly* inside `attachments_dir`.
///
/// Besides the name validation, the resolved path is canonicalized and must have the
/// canonical attachments dir as its immediate parent, so a symlink planted inside the
/// directory cannot be used to read files elsewhere.
pub fn resolve_attachment_path(
    attachments_dir: &Path,
    uri_path: &str,
) -> Result<(PathBuf, &'static str), ResolveError> {
    let name = file_name_from_uri_path(uri_path)?;
    let ext = validate_attachment_name(name)?;
    let mime = mime_for_extension(&ext).ok_or(ResolveError::Invalid)?;

    let canonical_dir = attachments_dir
        .canonicalize()
        .map_err(|_| ResolveError::NotFound)?;
    let canonical_file = canonical_dir
        .join(name)
        .canonicalize()
        .map_err(|_| ResolveError::NotFound)?;
    if canonical_file.parent() != Some(canonical_dir.as_path()) || !canonical_file.is_file() {
        return Err(ResolveError::NotFound);
    }
    Ok((canonical_file, mime))
}

/// Builds the HTTP response for a request to the attachment URI scheme.
pub fn build_attachment_response(
    attachments_dir: &Path,
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Method, Response, StatusCode, header};

    let plain = |status: StatusCode| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(status.canonical_reason().unwrap_or("").as_bytes().to_vec())
            .unwrap()
    };

    if request.method() != Method::GET && request.method() != Method::HEAD {
        return plain(StatusCode::METHOD_NOT_ALLOWED);
    }

    match resolve_attachment_path(attachments_dir, request.uri().path()) {
        Ok((path, mime)) => match std::fs::read(&path) {
            Ok(bytes) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .header("X-Content-Type-Options", "nosniff")
                // Neutralise scripts in e.g. SVG if the file is ever opened as a document.
                .header(
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; img-src data:; style-src 'unsafe-inline'; sandbox",
                )
                .header(header::CACHE_CONTROL, "private, max-age=31536000, immutable")
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .body(if request.method() == Method::HEAD { Vec::new() } else { bytes })
                .unwrap(),
            Err(_) => plain(StatusCode::NOT_FOUND),
        },
        Err(ResolveError::Invalid) => plain(StatusCode::BAD_REQUEST),
        Err(ResolveError::NotFound) => plain(StatusCode::NOT_FOUND),
    }
}

pub fn save_attachment_file(
    base_data_dir: &Path,
    file_name: &str,
    data_base64: &str,
) -> Result<String, String> {
    let attachments_dir = attachments_dir(base_data_dir);
    if let Err(e) = std::fs::create_dir_all(&attachments_dir) {
        return Err(format!("Failed to create attachments dir: {}", e));
    }

    // Determine extension safely, strictly allowing image & document types
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "png".to_string());

    let safe_ext = if ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        ext
    } else {
        "png".to_string()
    };

    let new_filename = format!("{}.{}", uuid::Uuid::new_v4(), safe_ext);
    let target_path = attachments_dir.join(&new_filename);

    // Verify canonical path does not escape attachments_dir
    // Strip data URL prefix if present (e.g. data:image/png;base64,...)
    let clean_base64 = if let Some(idx) = data_base64.find(";base64,") {
        &data_base64[idx + 8..]
    } else if let Some(stripped) = data_base64.strip_prefix("data:") {
        stripped
    } else {
        data_base64
    };

    let bytes = BASE64
        .decode(clean_base64.trim())
        .map_err(|e| format!("Invalid base64 attachment data: {}", e))?;

    std::fs::write(&target_path, bytes)
        .map_err(|e| format!("Failed to write attachment file: {}", e))?;

    // Return the relative path: attachments/<uuid>.<ext>
    Ok(format!("{}{}", STORED_URL_PREFIX, new_filename))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_attachment_file_and_traversal_prevention() {
        let temp_dir = std::env::temp_dir().join(format!("sy_notes_attach_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Normal save with data URL prefix
        let raw = b"hello image bytes";
        let b64 = format!("data:image/png;base64,{}", BASE64.encode(raw));
        let rel_path = save_attachment_file(&temp_dir, "my_photo.png", &b64).expect("save success");

        assert!(rel_path.starts_with("attachments/"));
        assert!(rel_path.ends_with(".png"));

        let full_path = temp_dir.join(&rel_path);
        assert!(full_path.exists());
        let read_bytes = std::fs::read(&full_path).unwrap();
        assert_eq!(read_bytes, raw);

        // 2. Directory traversal attempt in file_name (e.g. "../../../etc/passwd")
        let evil_name = "../../../evil.exe";
        let evil_res = save_attachment_file(&temp_dir, evil_name, &b64).expect("save safely sanitized");
        assert!(evil_res.starts_with("attachments/"));
        assert!(evil_res.ends_with(".png"), "evil extension should default to safe ext");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    fn temp_attachments() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("sy_notes_serve_{}", uuid::Uuid::new_v4()));
        let dir = attachments_dir(&base);
        std::fs::create_dir_all(&dir).unwrap();
        (base, dir)
    }

    #[test]
    fn test_validate_attachment_name() {
        assert_eq!(validate_attachment_name("abc-123_x.png"), Ok("png".to_string()));
        assert_eq!(validate_attachment_name("A.JPG"), Ok("jpg".to_string()));
        for bad in [
            "", ".", "..", ".png", "a..png", "../a.png", "..\\a.png", "a/b.png", "a\\b.png",
            "%2e%2e%2fa.png", "a%2fb.png", "C:a.png", "a.png\0", "a.exe", "a", "a.", "a.png.exe",
            "a b.png", "\u{ff0e}\u{ff0e}.png",
        ] {
            assert_eq!(validate_attachment_name(bad), Err(ResolveError::Invalid), "{bad:?}");
        }
        assert_eq!(validate_attachment_name(&format!("{}.png", "a".repeat(300))), Err(ResolveError::Invalid));
    }

    #[test]
    fn test_file_name_from_uri_path() {
        assert_eq!(file_name_from_uri_path("/x.png"), Ok("x.png"));
        assert_eq!(file_name_from_uri_path("/attachments/x.png"), Ok("x.png"));
        assert_eq!(file_name_from_uri_path("x.png"), Ok("x.png"));
        for bad in ["/", "//x.png", "/../x.png", "/attachments/../x.png", "/a/x.png", "/attachments/a/x.png", "/%2e%2e/x.png"] {
            assert_eq!(file_name_from_uri_path(bad), Err(ResolveError::Invalid), "{bad:?}");
        }
    }

    #[test]
    fn test_resolve_attachment_path() {
        let (base, dir) = temp_attachments();
        std::fs::write(dir.join("ok.png"), b"png").unwrap();
        std::fs::write(base.join("secret.txt"), b"secret").unwrap();
        std::fs::create_dir_all(dir.join("sub.png")).unwrap();

        let (path, mime) = resolve_attachment_path(&dir, "/ok.png").unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(std::fs::read(path).unwrap(), b"png");

        assert_eq!(resolve_attachment_path(&dir, "/missing.png"), Err(ResolveError::NotFound));
        assert_eq!(resolve_attachment_path(&dir, "/sub.png"), Err(ResolveError::NotFound), "directories are not served");
        assert_eq!(resolve_attachment_path(&dir, "/../secret.txt"), Err(ResolveError::Invalid));
        assert_eq!(resolve_attachment_path(&base.join("nope"), "/ok.png"), Err(ResolveError::NotFound));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("secret.txt"), dir.join("link.txt")).unwrap();
            assert_eq!(resolve_attachment_path(&dir, "/link.txt"), Err(ResolveError::NotFound), "symlink escape");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_saved_attachment_round_trips_through_uri_scheme() {
        use tauri::http::{Request, StatusCode, header};
        let (base, dir) = temp_attachments();
        let stored = save_attachment_file(&base, "pic.webp", &BASE64.encode(b"webpdata")).unwrap();
        let file = stored.strip_prefix(STORED_URL_PREFIX).unwrap();

        let req = |method: &str, uri: &str| Request::builder().method(method).uri(uri).body(Vec::new()).unwrap();

        // Linux/macOS form and Windows/Android form of the same URL.
        for uri in [format!("attachment://localhost/{file}"), format!("http://attachment.localhost/{file}?v=1")] {
            let res = build_attachment_response(&dir, &req("GET", &uri));
            assert_eq!(res.status(), StatusCode::OK, "{uri}");
            assert_eq!(res.headers()[header::CONTENT_TYPE], "image/webp");
            assert_eq!(res.body(), b"webpdata");
        }

        let head = build_attachment_response(&dir, &req("HEAD", &format!("attachment://localhost/{file}")));
        assert_eq!(head.status(), StatusCode::OK);
        assert!(head.body().is_empty());

        std::fs::write(base.join("sy_notes.db"), b"db").unwrap();
        for (uri, status) in [
            ("attachment://localhost/..%2Fsy_notes.db", StatusCode::BAD_REQUEST),
            ("attachment://localhost/%2e%2e/sy_notes.db", StatusCode::BAD_REQUEST),
            ("attachment://localhost/../sy_notes.db", StatusCode::BAD_REQUEST),
            ("attachment://localhost/sy_notes.db", StatusCode::BAD_REQUEST),
            ("attachment://localhost/nope.png", StatusCode::NOT_FOUND),
        ] {
            assert_eq!(build_attachment_response(&dir, &req("GET", uri)).status(), status, "{uri}");
        }
        assert_eq!(
            build_attachment_response(&dir, &req("POST", &format!("attachment://localhost/{file}"))).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
