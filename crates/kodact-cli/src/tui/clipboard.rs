//! Image loading (`--image`) and cross-platform clipboard image paste (`Ctrl+V` / `Alt+V`).

use std::path::Path;
use std::process::Command;

use base64::Engine as _;
use kodact_types::{ContentBlock, MediaSource};

/// Maximum number of images attached to a single prompt.
pub const MAX_IMAGES: usize = 4;

/// Maximum raw byte size per attached image (5 MB).
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// An image ready to be attached as a [`ContentBlock::Image`] on the next user turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedImage {
    pub name: String,
    pub media_type: String,
    /// Base64-encoded image bytes.
    pub data: String,
    /// Original raw byte length.
    pub bytes: usize,
    /// Optional `(width, height)` parsed from the image header.
    pub dims: Option<(u32, u32)>,
}

impl AttachedImage {
    /// Load and validate an image from a local file path (`--image <path>`).
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let media_type = media_type_for(path).ok_or_else(|| {
            format!("unsupported image format: {} (expected png, jpg, jpeg, webp, or gif)", path.display())
        })?;
        let raw = std::fs::read(path).map_err(|e| format!("cannot read image {}: {e}", path.display()))?;
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("image").to_string();
        Self::from_bytes(name, media_type.to_string(), &raw)
    }

    /// Build an attachment from raw image bytes (for example, PNG bytes from the clipboard).
    pub fn from_bytes(name: String, media_type: String, raw: &[u8]) -> Result<Self, String> {
        if raw.is_empty() {
            return Err("image is empty".to_string());
        }
        if raw.len() > MAX_IMAGE_BYTES {
            return Err(format!(
                "image `{name}` is {:.1} MB (limit is {} MB)",
                raw.len() as f64 / (1024.0 * 1024.0),
                MAX_IMAGE_BYTES / (1024 * 1024)
            ));
        }
        let dims = parse_dimensions(raw);
        let data = base64::engine::general_purpose::STANDARD.encode(raw);
        Ok(Self { name, media_type, data, bytes: raw.len(), dims })
    }

    /// Convert into a wire [`ContentBlock::Image`].
    pub fn to_content_block(&self) -> ContentBlock {
        ContentBlock::Image {
            source: MediaSource::Base64 { media_type: self.media_type.clone(), data: self.data.clone() },
            cache_control: None,
        }
    }

    /// Compact pill label: `img #1 · screenshot.png · 1280×720 · 64 KB`.
    pub fn pill_label(&self, index: usize) -> String {
        let size = format_bytes(self.bytes);
        match self.dims {
            Some((w, h)) => format!("img #{index} · {} · {w}×{h} · {size}", self.name),
            None => format!("img #{index} · {} · {size}", self.name),
        }
    }
}

fn format_bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn media_type_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// Zero-dependency header dimension parser for PNG, GIF, and JPEG.
pub fn parse_dimensions(raw: &[u8]) -> Option<(u32, u32)> {
    // PNG: 8-byte signature + IHDR chunk (width at 16..20, height at 20..24)
    if raw.len() >= 24 && &raw[0..8] == b"\x89PNG\r\n\x1a\n" {
        let w = u32::from_be_bytes([raw[16], raw[17], raw[18], raw[19]]);
        let h = u32::from_be_bytes([raw[20], raw[21], raw[22], raw[23]]);
        return Some((w, h));
    }
    // GIF: "GIF87a" or "GIF89a", width at 6..8, height at 8..10 (little-endian)
    if raw.len() >= 10 && (&raw[0..6] == b"GIF87a" || &raw[0..6] == b"GIF89a") {
        let w = u16::from_le_bytes([raw[6], raw[7]]) as u32;
        let h = u16::from_le_bytes([raw[8], raw[9]]) as u32;
        return Some((w, h));
    }
    // JPEG: scan SOF0/SOF2 markers (0xFFC0 / 0xFFC2)
    if raw.len() >= 4 && raw[0] == 0xFF && raw[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < raw.len() {
            if raw[i] != 0xFF {
                break;
            }
            let marker = raw[i + 1];
            if marker == 0xC0 || marker == 0xC2 {
                let h = u16::from_be_bytes([raw[i + 5], raw[i + 6]]) as u32;
                let w = u16::from_be_bytes([raw[i + 7], raw[i + 8]]) as u32;
                return Some((w, h));
            }
            if marker == 0xD9 || marker == 0xDA {
                break;
            }
            let len = u16::from_be_bytes([raw[i + 2], raw[i + 3]]) as usize;
            if len < 2 {
                break;
            }
            i += 2 + len;
        }
    }
    None
}

/// Best-effort read of a PNG image from the system clipboard.
pub fn read_clipboard_image() -> Result<AttachedImage, String> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = Command::new("pngpaste").arg("-").output() {
            if out.status.success() && !out.stdout.is_empty() {
                return AttachedImage::from_bytes("clipboard.png".into(), "image/png".into(), &out.stdout);
            }
        }
        let tmp = std::env::temp_dir().join(format!("kodact-clip-{}.png", std::process::id()));
        let script = format!(
            "try\nset d to the clipboard as «class PNGf»\nset f to open for access POSIX file \"{}\" with write permission\nset eof f to 0\nwrite d to f\nclose access f\nend try",
            tmp.display()
        );
        let _ = Command::new("osascript").arg("-e").arg(&script).output();
        if let Ok(bytes) = std::fs::read(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            if !bytes.is_empty() {
                return AttachedImage::from_bytes("clipboard.png".into(), "image/png".into(), &bytes);
            }
        }
        Err("no image found in macOS clipboard".to_string())
    }
    #[cfg(target_os = "linux")]
    {
        for (cmd, args) in [
            ("wl-paste", &["--no-newline", "--type", "image/png"][..]),
            ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"][..]),
        ] {
            if let Ok(out) = Command::new(cmd).args(args).output() {
                if out.status.success() && !out.stdout.is_empty() {
                    return AttachedImage::from_bytes("clipboard.png".into(), "image/png".into(), &out.stdout);
                }
            }
        }
        Err("no image found in clipboard (requires wl-paste or xclip)".to_string())
    }
    #[cfg(target_os = "windows")]
    {
        let tmp = std::env::temp_dir().join(format!("kodact-clip-{}.png", std::process::id()));
        let ps = format!(
            "$img = Get-Clipboard -Format Image; if ($img) {{ $img.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png) }}",
            tmp.display()
        );
        let _ = Command::new("powershell.exe").args(["-NoProfile", "-Command", &ps]).output();
        if let Ok(bytes) = std::fs::read(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            if !bytes.is_empty() {
                return AttachedImage::from_bytes("clipboard.png".into(), "image/png".into(), &bytes);
            }
        }
        Err("no image found in Windows clipboard".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err("clipboard image paste is not supported on this platform".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_png_and_gif_dimensions_and_rejects_bad_files() {
        // Minimal 24-byte PNG header with 842 x 512 dimensions.
        let mut png = Vec::from(&b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR"[..]);
        png.extend_from_slice(&842u32.to_be_bytes());
        png.extend_from_slice(&512u32.to_be_bytes());
        assert_eq!(parse_dimensions(&png), Some((842, 512)));

        let img = AttachedImage::from_bytes("shot.png".into(), "image/png".into(), &png).unwrap();
        assert_eq!(img.dims, Some((842, 512)));
        assert_eq!(img.pill_label(1), "img #1 · shot.png · 842×512 · 24 B");
        assert!(matches!(img.to_content_block(), ContentBlock::Image { .. }));

        // GIF89a 320x200
        let mut gif = Vec::from(&b"GIF89a"[..]);
        gif.extend_from_slice(&320u16.to_le_bytes());
        gif.extend_from_slice(&200u16.to_le_bytes());
        assert_eq!(parse_dimensions(&gif), Some((320, 200)));

        // Unsupported extension rejected
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("notes.txt");
        std::fs::write(&bad, "hi").unwrap();
        assert!(AttachedImage::from_path(&bad).is_err());

        // Oversize rejected
        let huge = vec![0u8; MAX_IMAGE_BYTES + 1];
        assert!(AttachedImage::from_bytes("big.png".into(), "image/png".into(), &huge).is_err());
    }
}
