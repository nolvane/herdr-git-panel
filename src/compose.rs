//! Writing text for a forge from the list: a text field of several lines
//! and what it is for (a reply to a thread, a comment on the merge request).
//! The field knows nothing about forges or drawing, so other writes that
//! need text can reuse it.

use crate::model::MergeRequest;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

/// What a key did to the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Changed,
    /// `Ctrl+s`: the text is done.
    Finish,
    /// `Esc`: throw the text away.
    Discard,
    Ignored,
}

/// Spaces a tab becomes, in typed and in pasted text.
const TAB: &str = "    ";

/// A text field of several lines: typed characters are added at the end,
/// `Enter` adds a line break, `Backspace` deletes the last character. There
/// is no cursor to move.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextField {
    pub text: String,
}

impl TextField {
    pub fn key(&mut self, code: KeyCode, mods: KeyModifiers) -> Edit {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Char('s') if ctrl => Edit::Finish,
            KeyCode::Esc => Edit::Discard,
            KeyCode::Enter => {
                self.text.push('\n');
                Edit::Changed
            }
            KeyCode::Tab => {
                self.text.push_str(TAB);
                Edit::Changed
            }
            KeyCode::Backspace => {
                self.text.pop();
                Edit::Changed
            }
            // With Alt too: some keyboards type `@` or `|` with Option.
            KeyCode::Char(c) if !ctrl && !c.is_control() => {
                self.text.push(c);
                Edit::Changed
            }
            _ => Edit::Ignored,
        }
    }

    /// Adds pasted text as it is, with its line breaks. Terminals paste
    /// `\r` for a line break; other control characters are dropped, since
    /// they would act on the terminal when drawn.
    pub fn paste(&mut self, pasted: &str) {
        let text = pasted.replace("\r\n", "\n").replace('\r', "\n");
        for c in text.chars() {
            match c {
                '\n' => self.text.push('\n'),
                '\t' => self.text.push_str(TAB),
                c if c.is_control() => {}
                c => self.text.push(c),
            }
        }
    }

    /// Nothing but spaces and line breaks.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// What the text is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A reply to a thread: the forge's id of the thread and where it is.
    Reply {
        thread: String,
        place: Option<String>,
    },
    /// A comment on the merge request itself.
    Comment,
}

/// Where the text is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Writing,
    /// Shown as it will be posted; waits for `y`.
    Preview,
    /// Sent; waits for the forge.
    Posting,
}

/// Text being written for a merge request.
#[derive(Debug, Clone, PartialEq)]
pub struct Compose {
    /// The merge request as it was when the field opened, so a refresh
    /// while writing cannot send the text somewhere else.
    pub mr: MergeRequest,
    pub target: Target,
    pub field: TextField,
    pub stage: Stage,
    /// Why the last attempt did not post, shown in the box until the text
    /// changes.
    pub message: Option<String>,
    /// How many lines the box is scrolled up from the end of the text.
    pub scroll_up: u16,
    /// Lines that do not fit the box, set while drawing.
    pub hidden: u16,
}

impl Compose {
    pub fn new(mr: MergeRequest, target: Target) -> Compose {
        Compose {
            mr,
            target,
            field: TextField::default(),
            stage: Stage::Writing,
            message: None,
            scroll_up: 0,
            hidden: 0,
        }
    }

    /// Scrolls the box up (`lines` above 0) or down, within the text.
    pub fn scroll(&mut self, lines: i32) {
        let up = (self.scroll_up as i32 + lines).clamp(0, self.hidden as i32);
        self.scroll_up = up as u16;
    }

    /// "reply" or "comment".
    pub fn noun(&self) -> &'static str {
        match self.target {
            Target::Reply { .. } => "reply",
            Target::Comment => "comment",
        }
    }

    /// Where the text goes, for the box's title: "Reply to thread on
    /// src/a.rs:7 · !90" or "Comment on !90".
    pub fn title(&self) -> String {
        let r = self.mr.reference();
        match &self.target {
            Target::Reply {
                place: Some(place), ..
            } => format!("Reply to thread on {place} · {r}"),
            Target::Reply { place: None, .. } => format!("Reply to thread · {r}"),
            Target::Comment => format!("Comment on {r}"),
        }
    }

    /// The preview's question, with where the text goes: "Post reply to
    /// thread on src/a.rs:7 · !90?" or "Post comment on !90?".
    pub fn question(&self) -> String {
        let r = self.mr.reference();
        match &self.target {
            Target::Reply {
                place: Some(place), ..
            } => format!("Post reply to thread on {place} · {r}?"),
            Target::Reply { place: None, .. } => format!("Post reply to thread · {r}?"),
            Target::Comment => format!("Post comment on {r}?"),
        }
    }

    /// What the list says once the forge took it.
    pub fn done(&self) -> &'static str {
        match self.target {
            Target::Reply { .. } => "Reply posted",
            Target::Comment => "Comment posted",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(keys: &[KeyCode]) -> (TextField, Vec<Edit>) {
        let mut f = TextField::default();
        let edits = keys.iter().map(|k| f.key(*k, KeyModifiers::NONE)).collect();
        (f, edits)
    }

    #[test]
    fn enter_breaks_the_line_and_backspace_deletes() {
        let (f, _) = typed(&[
            KeyCode::Char('o'),
            KeyCode::Char('k'),
            KeyCode::Enter,
            KeyCode::Char('x'),
            KeyCode::Backspace,
            KeyCode::Char('y'),
        ]);
        assert_eq!(f.text, "ok\ny");
    }

    #[test]
    fn ctrl_s_finishes_and_esc_discards() {
        let mut f = TextField::default();
        assert_eq!(
            f.key(KeyCode::Char('s'), KeyModifiers::CONTROL),
            Edit::Finish
        );
        assert_eq!(f.key(KeyCode::Esc, KeyModifiers::NONE), Edit::Discard);
        assert_eq!(f.key(KeyCode::Char('s'), KeyModifiers::NONE), Edit::Changed);
        // Other chords type nothing.
        assert_eq!(
            f.key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Edit::Ignored
        );
        assert_eq!(f.text, "s");
    }

    #[test]
    fn capitals_and_symbols_with_shift_or_alt_are_typed() {
        let mut f = TextField::default();
        f.key(KeyCode::Char('A'), KeyModifiers::SHIFT);
        f.key(KeyCode::Char('>'), KeyModifiers::SHIFT);
        f.key(KeyCode::Char('@'), KeyModifiers::ALT);
        assert_eq!(f.text, "A>@");
    }

    #[test]
    fn paste_keeps_line_breaks_and_drops_control_characters() {
        let mut f = TextField::default();
        f.paste("one\r\ntwo\rthree\n\tfour\u{1b}[2J\u{0}");
        assert_eq!(f.text, "one\ntwo\nthree\n    four[2J");
    }

    #[test]
    fn blank_is_only_spaces_and_line_breaks() {
        let mut f = TextField::default();
        assert!(f.is_blank());
        f.paste("  \n  ");
        assert!(f.is_blank());
        f.paste(".");
        assert!(!f.is_blank());
    }
}
