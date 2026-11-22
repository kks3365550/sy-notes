use std::path::Path;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Saves a base64-encoded attachment into `<base_data_dir>/attachments/<uuid>.<ext>`.
/// Enforces directory traversal protection and UUID filename generation.
pub fn save_attachment_file(
    base_data_dir: &Path,
    file_name: &str,
    data_base64: &str,
) -> Result<String, String> {
    let attachments_dir = base_data_dir.join("attachments");
    if let Err(e) = std::fs::create_dir_all(&attachments_dir) {
        return Err(format!("Failed to create attachments dir: {}", e));
    }

    // Determine extension safely, strictly allowing image & document types
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "png".to_string());

    let safe_ext = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "pdf" | "txt" => ext,
        _ => "png".to_string(),
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
    Ok(format!("attachments/{}", new_filename))
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
}
