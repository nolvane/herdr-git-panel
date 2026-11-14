//! Text for the details panel: word wrapping of styled text with a hanging
//! indent, and a light Markdown renderer for merge request descriptions.

use crate::theme::{ACCENT, BLUE, FAINT, MUTED, TEXT, YELLOW};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// A link in wrapped text: line index, column, width and target.
pub type LinkSpot = (usize, u16, u16, String);

/// Wraps styled text to `width` cells. The first line starts with `first`,
/// every further line with `rest` (a hanging indent). Words longer than a
/// line are cut.
pub fn wrap(
    spans: Vec<Span<'static>>,
    width: u16,
    first: Span<'static>,
    rest: Span<'static>,
) -> Vec<Line<'static>> {
    let tagged = spans.into_iter().map(|s| (s, None)).collect();
    wrap_tagged(tagged, width, first, rest).0
}

/// Like `wrap`, for spans that may be links: also returns where each piece
/// of a link ended up (line index within the result, column, width, URL).
pub fn wrap_tagged(
    spans: Vec<(Span<'static>, Option<String>)>,
    width: u16,
    first: Span<'static>,
    rest: Span<'static>,
) -> (Vec<Line<'static>>, Vec<LinkSpot>) {
    let width = width as usize;
    // Words with their style and link; a word keeps the space after it.
    let mut words: Vec<(String, Style, Option<String>)> = Vec::new();
    for (span, link) in spans {
        let mut word = String::new();
        for ch in span.content.chars() {
            word.push(ch);
            if ch == ' ' {
                words.push((std::mem::take(&mut word), span.style, link.clone()));
            }
        }
        if !word.is_empty() {
            words.push((word, span.style, link.clone()));
        }
    }

    let mut lines = Vec::new();
    let mut spots: Vec<LinkSpot> = Vec::new();
    let mut current: Vec<Span<'static>> = vec![first.clone()];
    let mut used = first.content.width();
    let indent = rest.content.width();
    let mut push = |current: &mut Vec<Span<'static>>,
                    used: &mut usize,
                    lines_len: usize,
                    text: String,
                    style: Style,
                    link: &Option<String>| {
        let w = text.trim_end().width();
        if let Some(url) = link {
            spots.push((lines_len, *used as u16, w as u16, url.clone()));
        }
        *used += text.width();
        current.push(Span::styled(text, style));
    };
    // A word that does not end in a space runs on into the next one (a key
    // drawn in its own style next to "(" or ":"): a line breaks only where
    // a space is, so such a run moves to the next line whole.
    let mut runs = vec![0; words.len()];
    let mut start = 0;
    for (i, (word, _, _)) in words.iter().enumerate() {
        if word.ends_with(' ') || i + 1 == words.len() {
            let run = &words[start..=i];
            let full: usize = run.iter().map(|(w, _, _)| w.width()).sum();
            let trailing = word.width() - word.trim_end().width();
            runs[start] = full - trailing;
            start = i + 1;
        }
    }
    for (i, (word, style, link)) in words.into_iter().enumerate() {
        let w = word.trim_end().width();
        let run = runs[i];
        if run > 0 && used + run > width && used > indent.max(first.content.width()) {
            lines.push(Line::from(std::mem::take(&mut current)));
            current.push(rest.clone());
            used = indent;
        }
        if used + w > width {
            // A word longer than the line: cut it into pieces.
            let mut piece = String::new();
            for ch in word.chars() {
                let cw = ch.to_string().width();
                if used + piece.width() + cw > width && !piece.is_empty() {
                    let len = lines.len();
                    push(
                        &mut current,
                        &mut used,
                        len,
                        std::mem::take(&mut piece),
                        style,
                        &link,
                    );
                    lines.push(Line::from(std::mem::take(&mut current)));
                    current.push(rest.clone());
                    used = indent;
                }
                piece.push(ch);
            }
            let len = lines.len();
            push(&mut current, &mut used, len, piece, style, &link);
        } else {
            let len = lines.len();
            push(&mut current, &mut used, len, word, style, &link);
        }
    }
    lines.push(Line::from(current));
    (lines, spots)
}

/// Inline Markdown: **bold**, `code`, [links](url), bare URLs and `\/`. Links
/// carry their target. Everything else is shown as written.
fn inline(text: &str, base: Style) -> Vec<(Span<'static>, Option<String>)> {
    let link_style = Style::new().fg(BLUE).add_modifier(Modifier::UNDERLINED);
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    let flush = |plain: &mut String, spans: &mut Vec<(Span<'static>, Option<String>)>| {
        if !plain.is_empty() {
            spans.push((Span::styled(std::mem::take(plain), base), None));
        }
    };
    while let Some(ch) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                flush(&mut plain, &mut spans);
                spans.push((
                    Span::styled(after[..end].to_string(), base.add_modifier(Modifier::BOLD)),
                    None,
                ));
                rest = &after[end + 2..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                flush(&mut plain, &mut spans);
                spans.push((
                    Span::styled(after[..end].to_string(), Style::new().fg(YELLOW)),
                    None,
                ));
                rest = &after[end + 1..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('[') {
            if let Some(close) = after.find("](") {
                if let Some(end) = after[close + 2..].find(')') {
                    flush(&mut plain, &mut spans);
                    let url = &after[close + 2..close + 2 + end];
                    // Only web links can be clicked: a link written by someone
                    // else must not open a local file, app or other scheme.
                    let span = if is_web_link(url) {
                        (
                            Span::styled(after[..close].to_string(), link_style),
                            Some(url.to_string()),
                        )
                    } else {
                        (Span::styled(after[..close].to_string(), base), None)
                    };
                    spans.push(span);
                    rest = &after[close + 2 + end + 1..];
                    continue;
                }
            }
        }
        if rest.starts_with("https://") || rest.starts_with("http://") {
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '>' || c == '"')
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ')', ';', ':']);
            if url.len() > 8 {
                flush(&mut plain, &mut spans);
                spans.push((
                    Span::styled(url.to_string(), link_style),
                    Some(url.to_string()),
                ));
                rest = &rest[url.len()..];
                continue;
            }
        }
        // Backslashes before `/` as Markdown shows them: each pair is one
        // backslash, an odd last one escapes the `/` and is not shown
        // (`\/` is how the list sends a GitLab line starting with `/`).
        // Backslashes before anything else are shown as written.
        let run = rest.len() - rest.trim_start_matches('\\').len();
        if run > 0 {
            if rest[run..].starts_with('/') {
                plain.push_str(&"\\".repeat(run / 2));
                plain.push('/');
                rest = &rest[run + 1..];
            } else {
                plain.push_str(&rest[..run]);
                rest = &rest[run..];
            }
            continue;
        }
        plain.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    flush(&mut plain, &mut spans);
    spans
}

/// Renders a Markdown description into wrapped lines: headings bold and
/// without `#`, bullets as `•`, quotes with a bar, code blocks indented.
/// Also returns where the links are (line index, column, width, URL).
pub fn markdown_links(text: &str, width: u16) -> (Vec<Line<'static>>, Vec<LinkSpot>) {
    let mut spots: Vec<LinkSpot> = Vec::new();
    let body = Style::new().fg(TEXT);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.extend(wrap(
                vec![Span::styled(line.to_string(), Style::new().fg(MUTED))],
                width,
                Span::raw("  "),
                Span::raw("  "),
            ));
            continue;
        }
        if trimmed.is_empty() {
            if out.last().is_some_and(|l| l.width() > 0) {
                out.push(Line::from(""));
            }
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            if out.last().is_some_and(|l| l.width() > 0) {
                out.push(Line::from(""));
            }
            let style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
            add(
                &mut out,
                &mut spots,
                inline(trimmed[hashes..].trim(), style),
                width,
                Span::raw(""),
                Span::raw(""),
            );
            continue;
        }
        let indent = line.len() - trimmed.len();
        let pad = " ".repeat(indent.min(8));
        if let Some(item) = ["- ", "* ", "+ "]
            .iter()
            .find_map(|p| trimmed.strip_prefix(p))
        {
            let (mark, item) = match item.strip_prefix("[ ] ") {
                Some(rest) => ("☐ ", rest),
                None => match item
                    .strip_prefix("[x] ")
                    .or_else(|| item.strip_prefix("[X] "))
                {
                    Some(rest) => ("☑ ", rest),
                    None => ("• ", item),
                },
            };
            add(
                &mut out,
                &mut spots,
                inline(item, body),
                width,
                Span::styled(format!("{pad}{mark}"), Style::new().fg(ACCENT)),
                Span::raw(format!("{pad}  ")),
            );
            continue;
        }
        let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && trimmed[digits..].starts_with(". ") {
            let number = &trimmed[..digits + 2];
            add(
                &mut out,
                &mut spots,
                inline(&trimmed[digits + 2..], body),
                width,
                Span::styled(format!("{pad}{number}"), Style::new().fg(ACCENT)),
                Span::raw(" ".repeat(pad.len() + number.len())),
            );
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix('>') {
            let style = Style::new().fg(MUTED).add_modifier(Modifier::ITALIC);
            add(
                &mut out,
                &mut spots,
                inline(quote.trim_start(), style),
                width,
                Span::styled("│ ", Style::new().fg(FAINT)),
                Span::styled("│ ", Style::new().fg(FAINT)),
            );
            continue;
        }
        if trimmed.starts_with('|') {
            // Tables stay as written, dimmed, without wrapping into a mess.
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::new().fg(MUTED),
            )));
            continue;
        }
        add(
            &mut out,
            &mut spots,
            inline(line, body),
            width,
            Span::raw(""),
            Span::raw(""),
        );
    }
    while out.last().is_some_and(|l| l.width() == 0) {
        out.pop();
    }
    (out, spots)
}

/// Wraps tagged spans and appends them, shifting the link spots to the
/// lines' place in `out`.
fn add(
    out: &mut Vec<Line<'static>>,
    spots: &mut Vec<LinkSpot>,
    spans: Vec<(Span<'static>, Option<String>)>,
    width: u16,
    first: Span<'static>,
    rest: Span<'static>,
) {
    let (lines, s) = wrap_tagged(spans, width, first, rest);
    let base = out.len();
    spots.extend(s.into_iter().map(|(l, x, w, u)| (l + base, x, w, u)));
    out.extend(lines);
}

/// Whether the target is an `http` or `https` address the browser may open,
/// without spaces or control characters that `open` could read differently.
pub fn is_web_link(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_word_in_several_spans_wraps_whole() {
        use ratatui::style::{Modifier, Style};
        let key = Style::new().add_modifier(Modifier::BOLD);
        let lines = wrap(
            vec![
                Span::raw("send it (then "),
                Span::styled("y", key),
                Span::raw(") now"),
            ],
            15,
            Span::raw(""),
            Span::raw(""),
        );
        assert_eq!(text(&lines), ["send it (then ", "y) now"]);
        let lines = wrap(
            vec![
                Span::raw("all ("),
                Span::styled("a", key),
                Span::raw(" see)"),
            ],
            5,
            Span::raw(""),
            Span::raw(""),
        );
        assert_eq!(text(&lines), ["all ", "(a ", "see)"]);
    }

    #[test]
    fn wraps_with_hanging_indent() {
        let lines = wrap(
            vec![Span::raw("one two three four")],
            12,
            Span::raw("ab "),
            Span::raw("   "),
        );
        assert_eq!(text(&lines), ["ab one two ", "   three ", "   four"]);
    }

    #[test]
    fn cuts_long_words() {
        let lines = wrap(
            vec![Span::raw("abcdefghij")],
            6,
            Span::raw("> "),
            Span::raw("  "),
        );
        assert_eq!(text(&lines), ["> abcd", "  efgh", "  ij"]);
    }

    #[test]
    fn only_web_links_can_be_clicked() {
        let (lines, spots) = markdown_links(
            "[CI log](file:///System/Applications/Utilities/Terminal.app) \
             [a](-aCalculator) [b](smb://host/share) [c](/acme/app/-/jobs/1) \
             [d](https://x.test/ok) [e](https://x.test/a b)",
            200,
        );
        assert_eq!(text(&lines), ["CI log a b c d e"]);
        assert_eq!(spots, vec![(0, 13, 1, "https://x.test/ok".to_string())]);
        assert!(is_web_link("http://x.test/1"));
        for bad in [
            "file:///etc",
            "-a",
            "ssh://x",
            "https://a b",
            "https://x\u{1b}]",
        ] {
            assert!(!is_web_link(bad), "{bad}");
        }
    }

    #[test]
    fn links_are_found_and_placed() {
        let (lines, spots) =
            markdown_links("See [ticket](https://x.test/1) and https://y.test/2.", 80);
        assert_eq!(text(&lines), ["See ticket and https://y.test/2."]);
        assert_eq!(
            spots,
            vec![
                (0, 4, 6, "https://x.test/1".to_string()),
                (0, 15, 16, "https://y.test/2".to_string())
            ]
        );
    }

    #[test]
    fn an_escaped_slash_shows_as_markdown_shows_it() {
        let t = |md: &str| text(&markdown_links(md, 80).0);
        assert_eq!(t("\\/merge later"), ["/merge later"]);
        assert_eq!(t("\\/a and \\/b, not a\\b"), ["/a and /b, not a\\b"]);
        // An escaped backslash before `/` shows one backslash.
        assert_eq!(t("\\\\/x and \\\\\\/y"), ["\\/x and \\/y"]);
        // Other escapes are shown as written, as before.
        assert_eq!(t("\\*x\\ and \\"), ["\\*x\\ and \\"]);
        // Code keeps its backslash, as on the forges' pages.
        assert_eq!(t("run `\\/x` now"), ["run \\/x now"]);
        assert_eq!(
            t("```\n\\/usr/bin/env\n```\n\\/after"),
            ["  \\/usr/bin/env", "/after"]
        );
        assert_eq!(t("- \\/one\n## \\/two"), ["• /one", "", "/two"]);
    }

    #[test]
    fn markdown_basics() {
        let md = "## Ticket\n[OPS-1](https://x/y) fixes **it**.\n\n- one `a`\n  - two\n1. first\n> quoted\n```\ncode\n```";
        let t = text(&markdown_links(md, 40).0);
        assert_eq!(
            t,
            [
                "Ticket",
                "OPS-1 fixes it.",
                "",
                "• one a",
                "  • two",
                "1. first",
                "│ quoted",
                "  code"
            ]
        );
    }
}
