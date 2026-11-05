//! Files and images attached to a message, the way pi's terminal UI does it:
//! the message carries a path and the model reads the file with its tools.
//!
//! - A pasted image is written to `$TMPDIR/pi-clipboard-<id>.<ext>`, exactly
//!   where pi's terminal UI puts it.
//! - A pasted or dropped file gets a temporary symlink,
//!   `$TMPDIR/pi-gpui-attachments/<id>/<name>`, so the path in the message is
//!   short and stable while the original stays where it is.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    /// Shown in the message field as `[label]`; unique within the field.
    pub label: String,
    /// What the message sends.
    pub path: PathBuf,
    pub kind: AttachmentKind,
}

impl Attachment {
    pub fn token(&self) -> String {
        format!("[{}]", self.label)
    }
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// A temporary symlink to `original`, keeping its file name.
pub fn link_file(original: &Path) -> std::io::Result<PathBuf> {
    let name = original
        .file_name()
        .ok_or_else(|| std::io::Error::other("the path has no file name"))?;
    let directory = std::env::temp_dir()
        .join("pi-gpui-attachments")
        .join(unique_id());
    std::fs::create_dir_all(&directory)?;
    let link = directory.join(name);
    std::os::unix::fs::symlink(original, &link)?;
    Ok(link)
}

/// Writes a clipboard image where pi's terminal UI writes it. TIFF and BMP
/// (what macOS screenshots often put on the clipboard) become PNG, which
/// every model and pi's `read` tool understand.
pub fn save_clipboard_image(image: &gpui::Image) -> std::io::Result<PathBuf> {
    use gpui::ImageFormat;
    let extension = match image.format {
        ImageFormat::Png | ImageFormat::Tiff | ImageFormat::Bmp => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
    };
    let path = std::env::temp_dir().join(format!("pi-clipboard-{}.{extension}", unique_id()));
    match image.format {
        ImageFormat::Tiff | ImageFormat::Bmp => {
            let source = path.with_extension(if image.format == ImageFormat::Tiff {
                "tiff"
            } else {
                "bmp"
            });
            std::fs::write(&source, &image.bytes)?;
            let converted = std::process::Command::new("sips")
                .args(["-s", "format", "png"])
                .arg(&source)
                .arg("--out")
                .arg(&path)
                .output()?;
            let _ = std::fs::remove_file(&source);
            if !converted.status.success() {
                return Err(std::io::Error::other("could not convert the image to PNG"));
            }
        }
        _ => std::fs::write(&path, &image.bytes)?,
    }
    Ok(path)
}

/// Absolute paths of existing image files mentioned in `text`, in order.
/// Paths may contain spaces, so every `/` that starts a word is tried
/// against every image extension that follows it.
pub fn image_paths_in(text: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut search_from = 0;
    while let Some(relative) = text[search_from..].find('/') {
        let start = search_from + relative;
        search_from = start + 1;
        let starts_word = text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| character.is_whitespace() || "\"'(`".contains(character));
        if !starts_word {
            continue;
        }
        let rest = &text[start..];
        let mut best = None;
        for (index, _) in rest.match_indices('.') {
            let tail = &rest[index + 1..];
            let Some(extension) = IMAGE_EXTENSIONS
                .iter()
                .find(|extension| tail.to_ascii_lowercase().starts_with(*extension))
            else {
                continue;
            };
            let end = index + 1 + extension.len();
            let ends_word = rest[end..]
                .chars()
                .next()
                .is_none_or(|character| character.is_whitespace() || "\"').,`".contains(character));
            if ends_word && Path::new(&rest[..end]).is_file() {
                best = Some(end);
            }
            if rest[..index].contains('\n') {
                break;
            }
        }
        if let Some(end) = best {
            let path = PathBuf::from(&rest[..end]);
            if !found.contains(&path) {
                found.push(path);
            }
            search_from = start + end;
        }
    }
    found
}

fn unique_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!(
        "{:x}-{:x}-{:x}",
        nanos,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_keep_the_file_name_and_point_at_the_original() {
        let original = std::env::temp_dir().join(format!("pi-gpui-original-{}.txt", unique_id()));
        std::fs::write(&original, "hello").unwrap();
        let link = link_file(&original).unwrap();
        assert_eq!(link.file_name(), original.file_name());
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "hello");
        let _ = std::fs::remove_file(&original);
        let _ = std::fs::remove_dir_all(link.parent().unwrap());
    }

    #[test]
    fn finds_existing_image_paths_even_with_spaces() {
        let directory = std::env::temp_dir().join(format!("pi gpui images {}", unique_id()));
        std::fs::create_dir_all(&directory).unwrap();
        let image = directory.join("screen shot.png");
        std::fs::write(&image, b"png").unwrap();
        let text = format!("look at {} and /nope/missing.png, thanks", image.display());
        assert_eq!(image_paths_in(&text), vec![image.clone()]);
        assert!(is_image(&image));
        assert!(!is_image(Path::new("/tmp/notes.txt")));
        let pasted = std::env::temp_dir().join("pi-clipboard-abc.png");
        assert_eq!(
            display_text(&format!("see {} now", pasted.display())),
            "see [Image] now"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// An image shown in the transcript: a file on disk, or bytes that came
/// inline with a message (base64 image blocks).
#[derive(Clone)]
pub enum Picture {
    File(PathBuf),
    Data(std::sync::Arc<gpui::Image>),
}

impl Picture {
    pub fn source(&self) -> gpui::ImageSource {
        match self {
            Picture::File(path) => path.clone().into(),
            Picture::Data(image) => image.clone().into(),
        }
    }

    /// Image blocks of a message or tool result:
    /// `{"type": "image", "data": "<base64>", "mimeType": "image/png"}`.
    pub fn from_content(content: &serde_json::Value) -> Vec<Picture> {
        use base64::Engine as _;
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "image")
            .filter_map(|block| {
                let format = gpui::ImageFormat::from_mime_type(block["mimeType"].as_str()?)?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(block["data"].as_str()?)
                    .ok()?;
                Some(Picture::Data(std::sync::Arc::new(gpui::Image::from_bytes(
                    format, bytes,
                ))))
            })
            .collect()
    }

    /// Pictures of a user message: inline image blocks, then image files
    /// whose paths appear in its text.
    pub fn of_message(content: &serde_json::Value, text: &str) -> Vec<Picture> {
        let mut pictures = Self::from_content(content);
        pictures.extend(image_paths_in(text).into_iter().map(Picture::File));
        pictures
    }
}

/// Text for display: paths of attachments this app (or pi's terminal UI)
/// created in the temp folder become `[Image]` or `[file name]`, as they
/// looked in the message field. The message itself keeps the real paths.
pub fn display_text(text: &str) -> String {
    let temp = std::env::temp_dir();
    let temp = temp.to_string_lossy();
    let temp = temp.trim_end_matches('/');
    if !text.contains(temp) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find(temp) {
        out.push_str(&rest[..position]);
        let after = &rest[position + temp.len()..];
        let after = after.strip_prefix('/').unwrap_or(after);
        let replaced = if after.starts_with("pi-clipboard-") {
            let end = after.find(char::is_whitespace).unwrap_or(after.len());
            Some(("[Image]".to_string(), end))
        } else if let Some(tail) = after.strip_prefix("pi-gpui-attachments/") {
            tail.split_once('/').and_then(|(id, name_and_rest)| {
                let directory = std::env::temp_dir().join("pi-gpui-attachments").join(id);
                let name = std::fs::read_dir(directory)
                    .ok()?
                    .flatten()
                    .next()?
                    .file_name()
                    .to_string_lossy()
                    .into_owned();
                name_and_rest.starts_with(&name).then(|| {
                    let consumed = "pi-gpui-attachments/".len() + id.len() + 1 + name.len();
                    (format!("[{name}]"), consumed)
                })
            })
        } else {
            None
        };
        match replaced {
            Some((label, consumed)) => {
                out.push_str(&label);
                rest = &after[consumed..];
            }
            None => {
                out.push_str(&rest[position..position + temp.len()]);
                rest = &rest[position + temp.len()..];
            }
        }
    }
    out.push_str(rest);
    out
}
