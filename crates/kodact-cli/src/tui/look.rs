//! The terminal UI's styles (`/theme <style>`, the `tuiStyle` setting): how
//! the front page, the chat's lines and the prompt are laid out. The colours
//! (`/theme dark|light|none`) are separate and apply to every style.

/// One style of the terminal UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Look {
    /// A framed welcome with the mark and what is connected; a timeline of steps.
    #[default]
    Frame,
    /// Centred, the name glowing lime; quiet ✓ steps and a glowing prompt.
    Glow,
    /// A card with the mark and the session; turns headed `you` and `kodact`.
    Card,
    /// Aligned left, past work numbered; times and voices down the side.
    Ledger,
    /// Readouts of the session and a header bar; approvals over the thread.
    Status,
    /// One prompt in the middle; plain steps and answers.
    Bare,
}

impl Look {
    pub const ALL: [Look; 6] = [Look::Frame, Look::Glow, Look::Card, Look::Ledger, Look::Status, Look::Bare];

    /// The style a setting names, if any (case does not matter). The names
    /// from before KodAct was renamed (`forgeframe`, `molten`, `anvil`) still work.
    pub fn named(s: &str) -> Option<Look> {
        let s = s.trim();
        let s = match s.to_ascii_lowercase().as_str() {
            "forgeframe" => "frame",
            "molten" => "glow",
            "anvil" => "card",
            _ => s,
        };
        Look::ALL.into_iter().find(|l| l.name().eq_ignore_ascii_case(s))
    }

    pub fn name(self) -> &'static str {
        match self {
            Look::Frame => "frame",
            Look::Glow => "glow",
            Look::Card => "card",
            Look::Ledger => "ledger",
            Look::Status => "status",
            Look::Bare => "bare",
        }
    }

    /// Steps are written as plain, quiet lines (`read src/a.rs`) rather than with a dot and a bold verb.
    pub fn plain_steps(self) -> bool {
        matches!(self, Look::Ledger | Look::Bare)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_are_found_by_name() {
        assert_eq!(Look::named("Glow"), Some(Look::Glow));
        assert_eq!(Look::named(" bare "), Some(Look::Bare));
        assert_eq!(Look::named("neon"), None);
        assert_eq!(Look::named("molten"), Some(Look::Glow), "the names from before the rename still work");
        assert_eq!(Look::named("forgeframe"), Some(Look::Frame));
        assert_eq!(Look::named("anvil"), Some(Look::Card));
        assert!(Look::ALL.iter().all(|l| Look::named(l.name()) == Some(*l)));
        // The names match the /theme command's list.
        let core: Vec<&str> = kodact_core::commands::STYLES.iter().map(|(n, _)| *n).collect();
        let ui: Vec<&str> = Look::ALL.iter().map(|l| l.name()).collect();
        assert_eq!(core, ui);
    }
}
