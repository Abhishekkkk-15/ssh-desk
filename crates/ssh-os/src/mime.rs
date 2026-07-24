//! Lightweight open-with routing by path extension / sniff.

use std::path::Path;

/// Suggested action when opening a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAction {
    ViewText,
    EditText,
    PreviewImage,
    Hex,
    OpenInPty,
}

/// Best-effort open routing from file name (content sniff comes later).
pub fn sniff_open_action(path: &Path) -> OpenAction {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => OpenAction::PreviewImage,
        "rs" | "toml" | "md" | "txt" | "json" | "yaml" | "yml" | "py" | "js" | "ts"
        | "go" | "c" | "h" | "cpp" | "hpp" | "sh" | "bash" | "zsh" | "css" | "html" | "xml"
        | "ini" | "cfg" | "conf" | "log" | "env" => OpenAction::EditText,
        "bin" | "so" | "o" | "a" | "exe" | "dll" | "wasm" => OpenAction::Hex,
        _ => OpenAction::ViewText,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn routes_common_types() {
        assert_eq!(
            sniff_open_action(&PathBuf::from("a.rs")),
            OpenAction::EditText
        );
        assert_eq!(
            sniff_open_action(&PathBuf::from("x.PNG")),
            OpenAction::PreviewImage
        );
        assert_eq!(
            sniff_open_action(&PathBuf::from("lib.so")),
            OpenAction::Hex
        );
    }
}
