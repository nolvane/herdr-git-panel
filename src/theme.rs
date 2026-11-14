//! Colours and icons.

use crate::model::{MergeState, Pipeline, ReviewState, Status};
use ratatui::style::Color;

pub const TEXT: Color = Color::Rgb(0xE4, 0xE4, 0xEF);
pub const MUTED: Color = Color::Rgb(0xA6, 0xAB, 0xCF);
pub const FAINT: Color = Color::Rgb(0x7A, 0x7F, 0xA3);
pub const BORDER: Color = Color::Rgb(0x3A, 0x3E, 0x5C);
pub const SELECTED_BG: Color = Color::Rgb(0x30, 0x33, 0x55);
pub const ACCENT: Color = Color::Rgb(0x9D, 0x8C, 0xF7);
pub const GREEN: Color = Color::Rgb(0x3F, 0xD2, 0x8A);
pub const RED: Color = Color::Rgb(0xF0, 0x56, 0x6D);
pub const YELLOW: Color = Color::Rgb(0xE8, 0xB9, 0x4E);
pub const BLUE: Color = Color::Rgb(0x6C, 0xA8, 0xF0);

pub struct Icons {
    pub nerd: bool,
}

impl Icons {
    fn pick(&self, nerd: &'static str, plain: &'static str) -> &'static str {
        if self.nerd {
            nerd
        } else {
            plain
        }
    }

    pub fn merge_state(&self, s: MergeState) -> (&'static str, Color) {
        match s {
            MergeState::Ready => (self.pick("\u{f419}", "✔"), ACCENT),
            MergeState::Draft => (self.pick("\u{f4dd}", "◌"), MUTED),
            MergeState::Conflict => (self.pick("\u{f421}", "!"), RED),
            MergeState::NeedsRebase => (self.pick("\u{f47f}", "↻"), YELLOW),
            MergeState::Checking => (self.pick("\u{f43a}", "…"), YELLOW),
            MergeState::Blocked => (self.pick("\u{f407}", "●"), GREEN),
        }
    }

    /// The icon of a merged or closed one, in place of its merge state;
    /// `None` for open ones.
    pub fn status(&self, s: Status) -> Option<(&'static str, Color)> {
        match s {
            Status::Open => None,
            // Octicons git-merge and git-pull-request-closed.
            Status::Merged => Some((self.pick("\u{f419}", "✔"), ACCENT)),
            Status::Closed => Some((self.pick("\u{f4dc}", "✕"), RED)),
        }
    }

    pub fn pipeline(&self, p: Pipeline) -> (&'static str, Color) {
        match p {
            Pipeline::None => (self.pick("\u{f48b}", "–"), FAINT),
            Pipeline::Success => (self.pick("\u{f4a4}", "✓"), GREEN),
            Pipeline::Failed => (self.pick("\u{f530}", "✗"), RED),
            Pipeline::Running => (self.pick("\u{f46a}", "…"), YELLOW),
            Pipeline::Canceled => (self.pick("\u{f517}", "⊘"), MUTED),
            Pipeline::Manual => (self.pick("\u{f4e3}", "⏸"), BLUE),
        }
    }

    pub fn review(&self, r: Option<ReviewState>) -> (&'static str, Color) {
        match r {
            None | Some(ReviewState::Unreviewed) => (self.pick("\u{f444}", "●"), YELLOW),
            Some(ReviewState::Started) => (self.pick("\u{f441}", "◐"), BLUE),
            Some(ReviewState::Reviewed) => (self.pick("\u{f442}", "◯"), MUTED),
            Some(ReviewState::ChangesRequested) => (self.pick("\u{f530}", "✗"), RED),
            Some(ReviewState::Approved) => (self.pick("\u{f4a4}", "✓"), GREEN),
        }
    }

    /// Left and right rounded ends for label pills (Powerline half circles).
    pub fn pill_caps(&self) -> Option<(&'static str, &'static str)> {
        self.nerd.then_some(("\u{e0b6}", "\u{e0b4}"))
    }

    pub fn threads(&self) -> &'static str {
        self.pick("\u{f442}", "#")
    }

    pub fn approvals(&self) -> &'static str {
        self.pick("\u{f42e}", "✓")
    }

    pub fn branch(&self) -> &'static str {
        self.pick("\u{f418}", "⎇")
    }

    pub fn repo(&self) -> &'static str {
        self.pick("\u{f401}", "▣")
    }

    /// The forge's logo, only with a Nerd Font (plain text has no logo).
    pub fn forge(&self, forge: crate::model::Forge) -> Option<&'static str> {
        self.nerd.then_some(match forge {
            crate::model::Forge::GitLab => "\u{f296}",
            crate::model::Forge::GitHub => "\u{f408}",
        })
    }

    pub fn refresh(&self) -> &'static str {
        self.pick("\u{f46a}", "↻")
    }
}

/// Stands before and after a key in a text of the list's own, so the key
/// is drawn bold and in the accent colour wherever the text ends up (see
/// `keys`). It has no width: wrapping and measuring a text do not see it.
pub const KEY_MARK: char = '\u{2060}';

/// Marks the keys in a text of the list's own: they are written between
/// backticks, as in "`a` shows all projects". Only for texts written here,
/// never for a title, a name, an error or anything else that came from a
/// forge, which may hold backticks of its own.
pub fn keys(text: &str) -> String {
    text.replace('`', "\u{2060}")
}

/// Text colour that reads on a label's background.
pub fn on(bg: Color) -> Color {
    match bg {
        Color::Rgb(r, g, b) => {
            let l = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
            if l > 150.0 {
                Color::Rgb(0x16, 0x17, 0x22)
            } else {
                Color::Rgb(0xF5, 0xF5, 0xFA)
            }
        }
        _ => TEXT,
    }
}

/// The footer bar.
pub const BAR_BG: Color = Color::Rgb(0x25, 0x28, 0x3E);

/// A label's colours as a soft pill: the colour darkened as background, a
/// light tint of it as text (instead of glaring full colour with black or
/// white text).
pub fn tint(c: Color) -> (Color, Color) {
    match c {
        Color::Rgb(r, g, b) => {
            let dark = |v: u8| (v as f32 * 0.6) as u8;
            let (r, g, b) = (dark(r), dark(g), dark(b));
            let light = |v: u8| (v as f32 + (255.0 - v as f32) * 0.65) as u8;
            (
                Color::Rgb(r, g, b),
                Color::Rgb(light(r), light(g), light(b)),
            )
        }
        other => (other, TEXT),
    }
}

/// `#rrggbb` to a colour.
pub fn hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_contrast() {
        assert_eq!(hex("#428BCA"), Some(Color::Rgb(0x42, 0x8B, 0xCA)));
        assert_eq!(hex("blue"), None);
        assert_eq!(on(Color::Rgb(255, 255, 255)), Color::Rgb(0x16, 0x17, 0x22));
        assert_eq!(on(Color::Rgb(0, 0, 0)), Color::Rgb(0xF5, 0xF5, 0xFA));
    }
}
