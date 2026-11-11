//! Shortcuts are written the macOS way in the code (`cmd-n`, `⌘N`). On Linux
//! the command key is Ctrl, so bindings and labels are translated here.

/// Shortcuts that would collide with a Linux habit get their own keys there:
/// (macOS binding, Linux binding, macOS label, Linux label).
const LINUX_OVERRIDES: &[(&str, &str, &str, &str)] = &[
    // Ctrl+Shift+C copies in every Linux terminal; it must not compact the session.
    ("cmd-shift-c", "ctrl-alt-c", "⌘⇧C", "Ctrl+Alt+C"),
];

/// A key binding: `cmd-shift-t` on macOS, `ctrl-shift-t` elsewhere.
pub fn binding(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.to_string()
    } else {
        linux_binding(keys)
    }
}

/// A shortcut label: `⌘⇧T` on macOS, `Ctrl+Shift+T` elsewhere.
pub fn label(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.to_string()
    } else {
        linux_label(keys)
    }
}

/// Shortcut labels inside a sentence, e.g. "… reload them (⌘⇧R)".
pub fn in_text(text: &str) -> String {
    if cfg!(target_os = "macos") {
        text.to_string()
    } else {
        linux_text(text)
    }
}

fn linux_binding(keys: &str) -> String {
    if let Some(found) = LINUX_OVERRIDES.iter().find(|entry| entry.0 == keys) {
        return found.1.to_string();
    }
    // A plugin may ask for `ctrl-cmd-x`; both are Ctrl here.
    keys.replace("cmd-", "ctrl-").replace("ctrl-ctrl-", "ctrl-")
}

fn linux_label(keys: &str) -> String {
    if let Some(found) = LINUX_OVERRIDES.iter().find(|entry| entry.2 == keys) {
        return found.3.to_string();
    }
    keys.split(' ')
        .map(|keystroke| {
            let (mut ctrl, mut alt, mut shift) = (false, false, false);
            let mut key = String::new();
            for character in keystroke.chars() {
                match character {
                    '⌘' | '⌃' => ctrl = true,
                    '⌥' => alt = true,
                    '⇧' => shift = true,
                    '↩' => key.push_str("Enter"),
                    other => key.push(other),
                }
            }
            let mut parts = Vec::new();
            if ctrl {
                parts.push("Ctrl");
            }
            if alt {
                parts.push("Alt");
            }
            if shift {
                parts.push("Shift");
            }
            if !key.is_empty() {
                parts.push(&key);
            }
            parts.join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Translates every run that starts with a modifier symbol and ends at the
/// next character that cannot be part of a shortcut.
fn linux_text(text: &str) -> String {
    let is_modifier = |character: char| "⌘⌃⌥⇧".contains(character);
    let mut result = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if !is_modifier(character) {
            result.push(character);
            continue;
        }
        let mut shortcut = String::from(character);
        while let Some(&next) = characters.peek() {
            if is_modifier(next) || next.is_alphanumeric() || next == '↩' || next == ',' {
                shortcut.push(next);
                characters.next();
            } else {
                break;
            }
        }
        result.push_str(&linux_label(&shortcut));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{linux_binding, linux_label, linux_text};

    #[test]
    fn linux_bindings() {
        assert_eq!(linux_binding("cmd-n"), "ctrl-n");
        assert_eq!(linux_binding("cmd-shift-t"), "ctrl-shift-t");
        assert_eq!(linux_binding("ctrl-cmd-x"), "ctrl-x");
        assert_eq!(linux_binding("escape"), "escape");
        // Not the terminal's copy shortcut.
        assert_eq!(linux_binding("cmd-shift-c"), "ctrl-alt-c");
    }

    #[test]
    fn linux_labels() {
        assert_eq!(linux_label("⌘N"), "Ctrl+N");
        assert_eq!(linux_label("⌘⇧T"), "Ctrl+Shift+T");
        assert_eq!(linux_label("⌥Enter"), "Alt+Enter");
        assert_eq!(linux_label("⌘↩"), "Ctrl+Enter");
        assert_eq!(linux_label("⌘1"), "Ctrl+1");
        assert_eq!(linux_label("⌘,"), "Ctrl+,");
        assert_eq!(linux_label("⌘⇧C"), "Ctrl+Alt+C");
    }

    #[test]
    fn linux_labels_in_sentences() {
        assert_eq!(
            linux_text("until you reload them (⌘⇧R)."),
            "until you reload them (Ctrl+Shift+R)."
        );
        assert_eq!(linux_text("no shortcut here"), "no shortcut here");
    }
}
