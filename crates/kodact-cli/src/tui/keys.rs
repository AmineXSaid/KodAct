//! Key rebinding: `<config>/keybindings.json` maps key specs to named actions
//! (docs/CLI.md, "Key bindings"), e.g.
//!
//! ```json
//! {"ctrl+s": "submit", "ctrl+g": "none", "alt+r": "historySearch"}
//! ```
//!
//! A binding is a translation: a key bound to an action arrives as that
//! action's default key, so the app's key handling has one set of keys to
//! know. `"none"` unbinds a key. Default keys keep working unless they are
//! bound to something else or to `"none"`. Bad entries are warnings at start,
//! never a failure.

use std::collections::BTreeMap;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key and its modifiers, normalized for comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Chord {
    code: ChordCode,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ChordCode {
    Char(char),
    Enter,
    Tab,
    Esc,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

impl Chord {
    /// The chord a key event is.
    pub fn of(k: &KeyEvent) -> Option<Chord> {
        let m = k.modifiers;
        let (code, shift) = match k.code {
            KeyCode::BackTab => (ChordCode::Tab, true),
            // A character's case already says whether Shift was down.
            KeyCode::Char(c) => (ChordCode::Char(c.to_ascii_lowercase()), c.is_ascii_uppercase()),
            KeyCode::Enter => (ChordCode::Enter, m.contains(KeyModifiers::SHIFT)),
            KeyCode::Tab => (ChordCode::Tab, m.contains(KeyModifiers::SHIFT)),
            KeyCode::Esc => (ChordCode::Esc, false),
            KeyCode::Backspace => (ChordCode::Backspace, false),
            KeyCode::Delete => (ChordCode::Delete, false),
            KeyCode::Up => (ChordCode::Up, m.contains(KeyModifiers::SHIFT)),
            KeyCode::Down => (ChordCode::Down, m.contains(KeyModifiers::SHIFT)),
            KeyCode::Left => (ChordCode::Left, m.contains(KeyModifiers::SHIFT)),
            KeyCode::Right => (ChordCode::Right, m.contains(KeyModifiers::SHIFT)),
            KeyCode::Home => (ChordCode::Home, false),
            KeyCode::End => (ChordCode::End, false),
            KeyCode::PageUp => (ChordCode::PageUp, false),
            KeyCode::PageDown => (ChordCode::PageDown, false),
            KeyCode::F(n) => (ChordCode::F(n), m.contains(KeyModifiers::SHIFT)),
            _ => return None,
        };
        Some(Chord { code, ctrl: m.contains(KeyModifiers::CONTROL), alt: m.contains(KeyModifiers::ALT), shift })
    }

    /// Parse a spec like `ctrl+r`, `alt+enter`, `shift+tab`, `esc`, `f2`, `?`.
    pub fn parse(spec: &str) -> Result<Chord, String> {
        let spec = spec.trim().to_ascii_lowercase();
        if spec.is_empty() {
            return Err("an empty key".into());
        }
        let parts: Vec<&str> = if spec == "+" { vec!["+"] } else { spec.split('+').collect() };
        let (key, mods) = parts.split_last().ok_or("an empty key")?;
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        for m in mods {
            match *m {
                "ctrl" | "control" => ctrl = true,
                "alt" | "meta" | "option" => alt = true,
                "shift" => shift = true,
                other => return Err(format!("unknown modifier {other:?}")),
            }
        }
        let code = match *key {
            "enter" | "return" => ChordCode::Enter,
            "tab" => ChordCode::Tab,
            "esc" | "escape" => ChordCode::Esc,
            "backspace" => ChordCode::Backspace,
            "delete" | "del" => ChordCode::Delete,
            "up" => ChordCode::Up,
            "down" => ChordCode::Down,
            "left" => ChordCode::Left,
            "right" => ChordCode::Right,
            "home" => ChordCode::Home,
            "end" => ChordCode::End,
            "pageup" => ChordCode::PageUp,
            "pagedown" => ChordCode::PageDown,
            "space" => ChordCode::Char(' '),
            k if k.len() > 1 && k.starts_with('f') && k[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => {
                ChordCode::F(k[1..].parse().unwrap_or(1))
            }
            k if k.chars().count() == 1 => ChordCode::Char(k.chars().next().unwrap_or(' ')),
            other => return Err(format!("unknown key {other:?}")),
        };
        Ok(Chord { code, ctrl, alt, shift })
    }

    /// The key event the app sees for this chord.
    fn event(&self) -> KeyEvent {
        let mut m = KeyModifiers::NONE;
        if self.ctrl {
            m |= KeyModifiers::CONTROL;
        }
        if self.alt {
            m |= KeyModifiers::ALT;
        }
        let code = match self.code {
            ChordCode::Tab if self.shift => return KeyEvent::new(KeyCode::BackTab, m | KeyModifiers::SHIFT),
            ChordCode::Char(c) if self.shift => KeyCode::Char(c.to_ascii_uppercase()),
            ChordCode::Char(c) => KeyCode::Char(c),
            ChordCode::Enter => KeyCode::Enter,
            ChordCode::Tab => KeyCode::Tab,
            ChordCode::Esc => KeyCode::Esc,
            ChordCode::Backspace => KeyCode::Backspace,
            ChordCode::Delete => KeyCode::Delete,
            ChordCode::Up => KeyCode::Up,
            ChordCode::Down => KeyCode::Down,
            ChordCode::Left => KeyCode::Left,
            ChordCode::Right => KeyCode::Right,
            ChordCode::Home => KeyCode::Home,
            ChordCode::End => KeyCode::End,
            ChordCode::PageUp => KeyCode::PageUp,
            ChordCode::PageDown => KeyCode::PageDown,
            ChordCode::F(n) => KeyCode::F(n),
        };
        if self.shift && !matches!(self.code, ChordCode::Char(_)) {
            m |= KeyModifiers::SHIFT;
        }
        KeyEvent::new(code, m)
    }

    /// The spec, as `parse` reads it.
    pub fn spec(&self) -> String {
        let mut s = String::new();
        for (on, name) in [(self.ctrl, "ctrl+"), (self.alt, "alt+"), (self.shift, "shift+")] {
            if on {
                s.push_str(name);
            }
        }
        let key = match self.code {
            ChordCode::Char(' ') => "space".to_string(),
            ChordCode::Char(c) => c.to_string(),
            ChordCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_ascii_lowercase(),
        };
        s + &key
    }
}

/// Named actions and their default keys. These are what `keybindings.json` binds.
pub const ACTIONS: &[(&str, &[&str], &str)] = &[
    ("submit", &["enter"], "Send the prompt"),
    ("newline", &["shift+enter", "alt+enter", "ctrl+j"], "New line"),
    ("cancel", &["esc"], "Interrupt the turn; close a menu or dialog; twice: /rewind"),
    ("interrupt", &["ctrl+c"], "Clear the input; interrupt; twice on an empty prompt: exit"),
    ("exit", &["ctrl+d"], "Exit (empty prompt)"),
    ("cycleMode", &["shift+tab"], "Cycle the permission mode"),
    ("historySearch", &["ctrl+r"], "Search earlier prompts"),
    ("complete", &["tab"], "Complete a / command or an @ path"),
    ("lineStart", &["ctrl+a", "home"], "Start of line"),
    ("lineEnd", &["ctrl+e", "end"], "End of line"),
    ("deleteWord", &["ctrl+w", "alt+backspace"], "Delete the word before the cursor"),
    ("killToStart", &["ctrl+u"], "Delete to the start of the line"),
    ("killToEnd", &["ctrl+k"], "Delete to the end of the line"),
    ("wordLeft", &["alt+b", "ctrl+left"], "Word left"),
    ("wordRight", &["alt+f", "ctrl+right"], "Word right"),
    ("redraw", &["ctrl+l"], "Redraw the screen"),
    ("toggleSidebar", &["ctrl+b"], "Show or hide the sidebar"),
    ("toggleDetails", &["ctrl+o"], "Show or hide thoughts and full tool output"),
    ("showKeys", &["?"], "Show the shortcuts (empty prompt)"),
];

fn action(name: &str) -> Option<&'static (&'static str, &'static [&'static str], &'static str)> {
    ACTIONS.iter().find(|(n, _, _)| *n == name)
}

/// The bindings in effect.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Keymap {
    /// The user's keys: to an action (`Some`) or unbound (`None`).
    user: BTreeMap<Chord, Option<&'static str>>,
}

impl Keymap {
    /// Read `<config>/keybindings.json`; a missing file is the defaults.
    pub fn load() -> (Keymap, Vec<String>) {
        Self::load_from(&kodact_config::config_dir().join("keybindings.json"))
    }

    pub fn load_from(path: &Path) -> (Keymap, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let (k, warnings) = Self::parse(&text);
                (k, warnings.into_iter().map(|w| format!("{}: {w}", path.display())).collect())
            }
            Err(_) => (Keymap::default(), vec![]),
        }
    }

    /// Parse the file's JSON: `{"<key>": "<action>" | "none"}`.
    pub fn parse(text: &str) -> (Keymap, Vec<String>) {
        let mut warnings = vec![];
        let mut k = Keymap::default();
        let v: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => return (k, vec![format!("not valid JSON ({e}); using the default keys")]),
        };
        let Some(obj) = v.as_object() else {
            return (k, vec!["expected an object of \"key\": \"action\"; using the default keys".into()]);
        };
        for (spec, a) in obj {
            let chord = match Chord::parse(spec) {
                Ok(c) => c,
                Err(e) => {
                    warnings.push(format!("{spec:?}: {e}; skipped"));
                    continue;
                }
            };
            let Some(name) = a.as_str() else {
                warnings.push(format!("{spec:?}: the action must be a string; skipped"));
                continue;
            };
            if name == "none" {
                k.user.insert(chord, None);
                continue;
            }
            let Some((name, _, _)) = action(name) else {
                let all: Vec<&str> = ACTIONS.iter().map(|a| a.0).collect();
                warnings
                    .push(format!("{spec:?}: unknown action {name:?} (actions: {}, none); skipped", all.join(", ")));
                continue;
            };
            // A default key bound to another action leaves its old one: say so.
            if let Some((old, _, _)) = ACTIONS
                .iter()
                .find(|(n, keys, _)| n != name && keys.iter().any(|d| Chord::parse(d).ok() == Some(chord)))
            {
                warnings.push(format!("{spec:?} was {old}; it is {name} now"));
            }
            k.user.insert(chord, Some(name));
        }
        (k, warnings)
    }

    /// The key the app should see for `key`, or `None` when it is unbound.
    pub fn translate(&self, key: KeyEvent) -> Option<KeyEvent> {
        let Some(chord) = Chord::of(&key) else { return Some(key) };
        match self.user.get(&chord) {
            Some(None) => None,
            Some(Some(name)) => {
                let (_, defaults, _) = action(name)?;
                Chord::parse(defaults[0]).ok().map(|c| c.event())
            }
            // Defaults keep working; a default the user gave to another action is matched above.
            None => Some(key),
        }
    }

    /// The keys that do `name` now: its defaults (unless unbound or taken), then the user's.
    pub fn keys_for(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = action(name)
            .map(|(_, d, _)| d.iter().copied())
            .into_iter()
            .flatten()
            .filter(|d| Chord::parse(d).map(|c| !self.user.contains_key(&c)).unwrap_or(true))
            .map(str::to_string)
            .collect();
        out.extend(self.user.iter().filter(|(_, a)| **a == Some(name)).map(|(c, _)| c.spec()));
        out
    }

    /// Keys the user unbound.
    pub fn unbound(&self) -> Vec<String> {
        self.user.iter().filter(|(_, a)| a.is_none()).map(|(c, _)| c.spec()).collect()
    }

    pub fn is_default(&self) -> bool {
        self.user.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn specs_parse_and_print() {
        for (spec, want) in [
            ("ctrl+r", "ctrl+r"),
            ("Ctrl+Shift+Tab", "ctrl+shift+tab"),
            ("alt+enter", "alt+enter"),
            ("esc", "esc"),
            ("F2", "f2"),
            ("?", "?"),
            ("space", "space"),
            ("meta+b", "alt+b"),
        ] {
            assert_eq!(Chord::parse(spec).unwrap().spec(), want, "{spec}");
        }
        assert!(Chord::parse("hyper+x").unwrap_err().contains("modifier"));
        assert!(Chord::parse("ctrl+banana").unwrap_err().contains("unknown key"));
        assert!(Chord::parse("").is_err());
        // Shift+Tab arrives as BackTab; an uppercase letter is shift+letter.
        assert_eq!(Chord::of(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)), Chord::parse("shift+tab").ok());
        assert_eq!(Chord::of(&ev(KeyCode::Char('R'), KeyModifiers::SHIFT)), Chord::parse("shift+r").ok());
    }

    #[test]
    fn bindings_translate_unbind_and_override() {
        let (k, w) =
            Keymap::parse(r#"{"ctrl+s": "submit", "ctrl+g": "none", "alt+r": "historySearch", "ctrl+a": "lineEnd"}"#);
        assert_eq!(w, vec!["\"ctrl+a\" was lineStart; it is lineEnd now".to_string()]);
        // A bound key arrives as the action's default key.
        assert_eq!(
            k.translate(ev(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            Some(ev(KeyCode::Enter, KeyModifiers::NONE))
        );
        assert_eq!(
            k.translate(ev(KeyCode::Char('r'), KeyModifiers::ALT)),
            Some(ev(KeyCode::Char('r'), KeyModifiers::CONTROL))
        );
        // "none" unbinds; defaults keep working; an overridden default does the new action.
        assert_eq!(k.translate(ev(KeyCode::Char('g'), KeyModifiers::CONTROL)), None);
        assert_eq!(
            k.translate(ev(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            Some(ev(KeyCode::Char('r'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            k.translate(ev(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            Some(ev(KeyCode::Char('e'), KeyModifiers::CONTROL))
        );
        // Plain typing passes through.
        assert_eq!(
            k.translate(ev(KeyCode::Char('x'), KeyModifiers::NONE)),
            Some(ev(KeyCode::Char('x'), KeyModifiers::NONE))
        );
        assert_eq!(k.keys_for("historySearch"), ["ctrl+r", "alt+r"]);
        assert_eq!(k.keys_for("lineStart"), ["home"], "ctrl+a went to lineEnd");
        assert_eq!(k.keys_for("lineEnd"), ["ctrl+e", "end", "ctrl+a"]);
        assert_eq!(k.unbound(), ["ctrl+g"]);
    }

    #[test]
    fn bad_entries_are_warnings() {
        let (k, w) = Keymap::parse(r#"{"ctrl+q": "teleport", "hyper+x": "submit", "ctrl+t": 3}"#);
        assert!(k.is_default());
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w.iter().any(|x| x.contains("unknown action \"teleport\"")));
        let (k, w) = Keymap::parse("{not json");
        assert!(k.is_default() && w[0].contains("not valid JSON"));
        let (_, w) = Keymap::parse("[1]");
        assert!(w[0].contains("expected an object"));
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Keymap::load_from(&d.path().join("missing.json")), (Keymap::default(), vec![]));
        std::fs::write(d.path().join("k.json"), r#"{"ctrl+q": "nope"}"#).unwrap();
        let (_, w) = Keymap::load_from(&d.path().join("k.json"));
        assert!(w[0].starts_with(&d.path().join("k.json").display().to_string()), "{w:?}");
    }
}
