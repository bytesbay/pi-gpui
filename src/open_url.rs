//! `pi-gpui://open?workspace=<dir>[&session=<file>]` links. The `/gpui`
//! command of the pi-gpui pi package opens one to move a terminal session into
//! the app; any other tool can do the same with `open "pi-gpui://open?..."`.

/// What an open link asks for: a project folder and, optionally, a session
/// file in it. Paths are as given; the caller checks that they exist.
#[derive(Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub workspace: String,
    pub session: Option<String>,
}

pub fn parse(url: &str) -> Option<OpenRequest> {
    let rest = url.strip_prefix("pi-gpui://open")?;
    let query = rest.trim_start_matches('/').strip_prefix('?').unwrap_or("");
    let mut workspace = None;
    let mut session = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value);
        if value.is_empty() {
            continue;
        }
        match key {
            "workspace" => workspace = Some(value),
            "session" => session = Some(value),
            _ => {}
        }
    }
    Some(OpenRequest {
        workspace: workspace?,
        session,
    })
}

/// `pi-gpui://open?workspace=<dir>` for a project folder.
/// Linux only: a second launch hands its folder to the running app.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn workspace_link(workspace: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;

    let mut link = String::from("pi-gpui://open?workspace=");
    for &byte in workspace.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            link.push(byte as char);
        } else {
            link.push_str(&format!("%{byte:02X}"));
        }
    }
    link
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%'
            && let Some(hex) = bytes.get(index + 1..index + 3)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            decoded.push(value);
            index += 3;
            continue;
        }
        decoded.push(if byte == b'+' { b' ' } else { byte });
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_workspace_and_session() {
        let request = parse(
            "pi-gpui://open?workspace=%2FUsers%2Fme%2Fmy%20app&session=%2Ftmp%2Fs%C3%A9ance.jsonl",
        )
        .unwrap();
        assert_eq!(request.workspace, "/Users/me/my app");
        assert_eq!(request.session.as_deref(), Some("/tmp/séance.jsonl"));
    }

    #[test]
    fn workspace_links_round_trip() {
        let link = workspace_link(std::path::Path::new("/home/me/my app+1"));
        assert_eq!(parse(&link).unwrap().workspace, "/home/me/my app+1");
    }

    #[test]
    fn session_is_optional() {
        let request = parse("pi-gpui://open/?workspace=/srv/x").unwrap();
        assert_eq!(request.workspace, "/srv/x");
        assert_eq!(request.session, None);
    }

    #[test]
    fn rejects_other_links() {
        assert_eq!(parse("pi-gpui://open?session=/tmp/a.jsonl"), None);
        assert_eq!(parse("pi-gpui://settings"), None);
        assert_eq!(parse("https://pi-gpui.dev"), None);
    }
}
