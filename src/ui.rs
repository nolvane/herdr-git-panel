//! Drawing: section tabs, the merge request table, a detail bar or the
//! details panel, and a footer. Records clickable areas for the mouse.

use crate::app::{Action, Activity, App, Hit, PanelTab, Section, ThreadFilter};
use crate::compose::{Compose, Stage};
use crate::model::{Label, MergeRequest, MergeState, Pipeline, ReviewState, Status};
use crate::text;
use crate::theme::{
    self, Icons, ACCENT, BAR_BG, BLUE, BORDER, FAINT, GREEN, MUTED, RED, SELECTED_BG, TEXT, YELLOW,
};
use crate::time;
use crate::view::{Show, SortKey};
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, HighlightSpacing, Paragraph, Row, Table};
use ratatui::Frame;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    State,
    Id,
    Title,
    /// Author (to review, all) or reviewers (mine).
    People,
    /// Your own review (to review, and all where you review).
    You,
    Pipeline,
    Approvals,
    Threads,
    Changes,
    Created,
    Updated,
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hits.clear();
    app.links.clear();
    let icons = Icons {
        nerd: app.config.nerd_font,
    };
    if app.diff.is_some() {
        draw_diff(frame, app, &icons);
        return;
    }
    // Header, a blank line, the list, the footer bar. Each merge request
    // says where it stands in its own second line.
    let height = frame.area().height;
    let spacer = if height >= 14 { 1 } else { 0 };
    let [header, _, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(spacer),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, &icons, header);
    if app.details && app.selected().is_some() {
        // Wide: the panel on the right, like a sidebar. Otherwise below.
        let (table_area, panel_area) = if body.width >= 140 {
            let panel = (body.width * 2 / 5).max(50);
            let [t, p] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Length(panel)]).areas(body);
            (t, p)
        } else {
            // The table only as tall as its rows need (at most half), the
            // panel takes the rest.
            let step = if app.config.compact { 1 } else { 2 };
            let wanted = app.rows(app.section).len() as u16 * step + 1;
            let table = wanted.clamp(5, (body.height / 2).max(5));
            let [t, p] =
                Layout::vertical([Constraint::Length(table), Constraint::Fill(1)]).areas(body);
            (t, p)
        };
        draw_table(frame, app, &icons, table_area);
        // A reply or comment is written at the bottom of the panel.
        let (panel_area, compose_area) = split_compose(app, panel_area);
        draw_details(frame, app, &icons, panel_area);
        if let Some(area) = compose_area {
            draw_compose(frame, app, area);
        }
    } else {
        // Also when its merge request left the list while writing.
        let (body, compose_area) = split_compose(app, body);
        draw_table(frame, app, &icons, body);
        if let Some(area) = compose_area {
            draw_compose(frame, app, area);
        }
    }
    draw_footer(frame, app, &icons, footer);
    if app.help {
        draw_help(frame, app);
    }
}

/// Room inside the text field's box: its border and one cell of padding on
/// each side.
const COMPOSE_FRAME: u16 = 4;

/// Takes the text field's box off the bottom of `area` while a reply or
/// comment is written: as tall as its text, at most half of `area`.
fn split_compose(app: &App, area: Rect) -> (Rect, Option<Rect>) {
    let Some(c) = &app.compose else {
        return (area, None);
    };
    let (lines, _) = compose_lines(c, area.width.saturating_sub(COMPOSE_FRAME));
    let wanted = lines.len() as u16 + 2;
    let height = wanted.clamp(4, (area.height / 2).max(4)).min(area.height);
    let [rest, field] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height)]).areas(area);
    (rest, Some(field))
}

/// The text as it is drawn in the box, wrapped at `width`, with the reason
/// of a failed attempt in red at the end; and where the cursor goes
/// (column, line) after the text.
fn compose_lines(c: &Compose, width: u16) -> (Vec<Line<'static>>, (u16, usize)) {
    let width = width.max(1);
    let style = Style::new().fg(TEXT);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for paragraph in c.field.text.split('\n') {
        if paragraph.is_empty() {
            lines.push(Line::from(""));
        } else {
            lines.extend(text::wrap(
                vec![Span::styled(paragraph.to_string(), style)],
                width,
                Span::raw(""),
                Span::raw(""),
            ));
        }
    }
    // The cursor after the last character; on the next line when that one
    // is full.
    let last = lines.last().map(|l| l.width() as u16).unwrap_or(0);
    let cursor = if last >= width {
        lines.push(Line::from(""));
        (0, lines.len() - 1)
    } else {
        (last, lines.len() - 1)
    };
    if c.field.text.is_empty() {
        lines[0] = Line::from(Span::styled(
            "⏎ new line · Ctrl+s send · Esc discard",
            Style::new().fg(FAINT),
        ));
    }
    if let Some(m) = &c.message {
        lines.extend(text::wrap(
            vec![Span::styled(m.clone(), Style::new().fg(RED))],
            width,
            Span::raw(""),
            Span::raw(""),
        ));
    }
    (lines, cursor)
}

/// The box a reply or comment is written in. Titled with where the text
/// goes; in the preview it shows the text as it will be posted.
fn draw_compose(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(c) = &app.compose else {
        return;
    };
    let (color, title) = match c.stage {
        Stage::Writing => (ACCENT, format!(" {} ", c.title())),
        Stage::Preview => (GREEN, format!(" {} · as it will be posted ", c.title())),
        Stage::Posting => (MUTED, format!(" {} · posting… ", c.title())),
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .padding(ratatui::widgets::Padding::horizontal(1))
        .title(Span::styled(
            cut(&title, area.width.saturating_sub(2) as usize),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    let (lines, (cx, cy)) = compose_lines(c, inner.width);
    // The end of the text stays in view, where the typing happens.
    let scroll = (lines.len() as u16).saturating_sub(inner.height);
    let writing = c.stage == Stage::Writing;
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
    let row = cy as u16;
    if writing && row >= scroll && row - scroll < inner.height {
        frame.set_cursor_position((inner.x + cx, inner.y + row - scroll));
    }
}

/// The changes viewer: a header with the merge request and the current
/// file, the diff with line numbers, and its keys.
fn draw_diff(frame: &mut Frame, app: &mut App, icons: &Icons) {
    use crate::diff::Kind;
    let area = frame.area();
    let [head, body, foot] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    // The file tree on the left on wide enough panes (f hides it).
    let show_tree = app.diff.as_ref().is_some_and(|v| v.tree) && body.width >= 90;
    let (tree_area, body) = if show_tree {
        let w = (body.width * 3 / 10).clamp(28, 48);
        let [t, _, b] = Layout::horizontal([
            Constraint::Length(w),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(body);
        (Some(t), b)
    } else {
        (None, body)
    };
    if let Some(t) = tree_area {
        draw_file_tree(frame, app, icons, t);
    }
    let Some(view) = app.diff.as_mut() else {
        return;
    };
    view.height = body.height as usize;
    let file_info = match &view.diff {
        Some(Ok(d)) => {
            let current = view.current_file();
            let path = current.map(|i| d.files[i].path.clone()).unwrap_or_default();
            vec![
                Span::styled(
                    format!(
                        " file {} of {}  ",
                        current.map(|i| i + 1).unwrap_or(0),
                        d.files.len()
                    ),
                    Style::new().fg(FAINT),
                ),
                Span::styled(path, Style::new().fg(TEXT)),
                Span::styled(format!("   +{}", d.added), Style::new().fg(GREEN)),
                Span::styled(format!(" -{}", d.removed), Style::new().fg(RED)),
            ]
        }
        _ => vec![],
    };
    let title = vec![
        Span::styled(
            format!(" {} Changes of {} ", icons.branch(), view.reference),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            cut(clean_title(&view.title), 60),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
    ];
    let [title_line, file_line] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(head);
    frame.render_widget(Paragraph::new(Line::from(title)), title_line);
    frame.render_widget(Paragraph::new(Line::from(file_info)), file_line);

    let mut lines: Vec<Line> = Vec::new();
    match &view.diff {
        None => lines.push(Line::from(Span::styled(
            " Loading the changes…",
            Style::new().fg(MUTED),
        ))),
        Some(Err(e)) => lines.push(Line::from(Span::styled(
            format!(" {e}"),
            Style::new().fg(RED),
        ))),
        Some(Ok(d)) if d.lines.is_empty() => lines.push(Line::from(Span::styled(
            " No changes.",
            Style::new().fg(MUTED),
        ))),
        Some(Ok(d)) => {
            let num = |n: Option<u32>| {
                n.map(|n| format!("{n:>5}"))
                    .unwrap_or_else(|| " ".repeat(5))
            };
            for l in d.lines.iter().skip(view.scroll).take(body.height as usize) {
                let line = match l.kind {
                    Kind::File => Line::from(vec![
                        Span::styled("▍", Style::new().fg(ACCENT).bg(SELECTED_BG)),
                        Span::styled(
                            format!(
                                " {} {:<width$}",
                                icons.repo(),
                                l.text,
                                width = body.width as usize
                            ),
                            Style::new()
                                .fg(TEXT)
                                .bg(SELECTED_BG)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Kind::Hunk => Line::from(Span::styled(
                        format!("{:>12}{}", "", l.text),
                        Style::new().fg(BLUE),
                    )),
                    Kind::Note => Line::from(Span::styled(
                        format!("{:>12}{}", "", l.text),
                        Style::new().fg(FAINT),
                    )),
                    Kind::Added | Kind::Removed | Kind::Context => {
                        let (sign, color) = match l.kind {
                            Kind::Added => ("+", GREEN),
                            Kind::Removed => ("-", RED),
                            _ => (" ", TEXT),
                        };
                        Line::from(vec![
                            Span::styled(
                                format!("{}{} ", num(l.old), num(l.new)),
                                Style::new().fg(FAINT),
                            ),
                            Span::styled(format!("{sign} {}", l.text), Style::new().fg(color)),
                        ])
                    }
                };
                lines.push(line);
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), body);

    // Keys, clickable.
    let keys: [(&str, &str, char); 6] = [
        ("↑↓", "scroll", ' '),
        ("n", "next file", 'n'),
        ("N", "previous file", 'N'),
        ("f", "file tree", 'f'),
        ("⏎", "open in browser", 'o'),
        ("q", "back to the list", 'q'),
    ];
    let mut spans = vec![Span::raw(" ")];
    let mut x = foot.x + 1;
    for (key, what, c) in keys {
        let text = format!(" {what}   ");
        let w = (key.chars().count() + text.chars().count()) as u16;
        if c != ' ' {
            app.hits.push((Rect::new(x, foot.y, w - 3, 1), Hit::Key(c)));
        }
        x += w;
        spans.push(Span::styled(
            key,
            Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(text, Style::new().fg(FAINT)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), foot);
}

/// The changed files as a tree beside the diff; the current file is
/// highlighted, a click shows a file.
fn draw_file_tree(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    let Some(view) = app.diff.as_ref() else {
        return;
    };
    let Some(Ok(d)) = &view.diff else {
        return;
    };
    let current = view.current_file();
    let rows = crate::diff::tree(&d.files);
    // Keep the current file in view.
    let current_row = rows
        .iter()
        .position(|r| r.file.is_some() && r.file == current)
        .unwrap_or(0);
    let height = area.height as usize;
    let offset = current_row.saturating_sub(height.saturating_sub(3));
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    for (n, r) in rows.iter().enumerate().skip(offset).take(height) {
        let indent = "  ".repeat(r.depth);
        let line = match r.file {
            None => Line::from(Span::styled(
                cut(&format!(" {indent}{}", r.name), area.width as usize),
                Style::new().fg(MUTED),
            )),
            Some(i) => {
                let f = &d.files[i];
                let counts = format!(" +{} -{}", f.added, f.removed);
                let room = (area.width as usize).saturating_sub(indent.len() + counts.len() + 3);
                let name = cut(&r.name, room.max(4));
                let pad = (area.width as usize)
                    .saturating_sub(indent.len() + 2 + name.chars().count() + counts.len());
                let is_current = current == Some(i);
                let style = if is_current {
                    Style::new().bg(SELECTED_BG)
                } else {
                    Style::new()
                };
                hits.push((
                    Rect::new(area.x, area.y + (n - offset) as u16, area.width, 1),
                    Hit::File(i),
                ));
                Line::from(vec![
                    Span::styled(if is_current { "▌" } else { " " }, Style::new().fg(ACCENT)),
                    Span::raw(format!("{indent} ")),
                    Span::styled(
                        name,
                        if is_current {
                            Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
                        } else {
                            Style::new().fg(TEXT)
                        },
                    ),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(format!(" +{}", f.added), Style::new().fg(GREEN)),
                    Span::styled(format!(" -{}", f.removed), Style::new().fg(RED)),
                ])
                .style(style)
            }
        };
        lines.push(line);
    }
    let _ = icons;
    frame.render_widget(Paragraph::new(lines), area);
    // A thin line between tree and diff.
    let sep = Rect::new(area.x + area.width, area.y, 1, area.height);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("│", Style::new().fg(BORDER)));
            area.height as usize
        ]),
        sep,
    );
    app.hits.extend(hits);
}

/// Every key, in a box over the list. Any key or click closes it.
fn draw_help(frame: &mut Frame, app: &mut App) {
    let groups: [(&'static str, &'static [(&'static str, &'static str)]); 5] = [
        (
            "Move",
            &[
                ("↑ ↓  j k", "move through the list"),
                ("Tab  1 2 3", "mine / to review / all"),
                ("g G", "first / last"),
                ("a", "this project / all projects"),
                ("z", "open / merged / closed"),
            ],
        ),
        (
            "Merge request or pull request",
            &[
                ("⏎  o", "open in the browser"),
                ("p  Space", "details panel on / off"),
                ("c", "panel: overview / activity"),
                ("Ctrl+d Ctrl+u", "scroll the panel"),
                ("d", "show the changes (n N files, f tree, q back)"),
                ("b", "open the branch in a new Herdr workspace"),
                ("v", "approve / take approval back"),
                ("m", "merge (when GitLab or GitHub allows it)"),
                ("w", "your own: draft / ready for review"),
                ("", "b v m w: open ones only"),
                ("y", "copy the link"),
            ],
        ),
        (
            "Activity (c)",
            &[
                ("t", "all / open / resolved threads"),
                ("n N", "select the next / previous thread"),
                ("e", "resolve the thread / open it again"),
                (">", "reply to the selected thread"),
                ("+", "write a comment"),
                ("", "> +: open ones only"),
                ("⏎  Ctrl+s  Esc", "in the text: new line / send / discard"),
            ],
        ),
        (
            "List",
            &[
                ("/", "search"),
                ("f", "open ones: all, needs me, threads, failed, ready"),
                ("s", "sort by the next key"),
                ("x", "clear search and filter"),
                ("r", "refresh now"),
                ("i", "Nerd Font icons / plain symbols"),
                ("Esc", "close panel, search or filter (never the list)"),
                ("q", "close the list (asks first)"),
            ],
        ),
        (
            "Mouse",
            &[
                ("click", "select, tab, heading sorts"),
                ("double-click", "open in the browser"),
                ("wheel", "move, or scroll the panel"),
            ],
        ),
    ];
    type Group = (&'static str, &'static [(&'static str, &'static str)]);
    let group_lines = |groups: &[Group]| -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, (title, keys)) in groups.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                *title,
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            )));
            for (k, what) in keys.iter() {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {k:<15}"),
                        Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(*what, Style::new().fg(MUTED)),
                ]));
            }
        }
        lines
    };
    let closing = || {
        Line::from(Span::styled(
            "Any key or click closes this.",
            Style::new().fg(FAINT),
        ))
    };
    const COLUMN: u16 = 64;
    let full = frame.area();
    let mut one = group_lines(&groups);
    one.extend([Line::from(""), closing()]);
    // Two columns side by side when one is taller than the screen and the
    // screen is wide enough.
    let two = one.len() as u16 + 2 > full.height && full.width >= 2 * COLUMN;
    let columns: Vec<Vec<Line>> = if two {
        let mut right = group_lines(&groups[2..]);
        right.extend([Line::from(""), closing()]);
        vec![group_lines(&groups[..2]), right]
    } else {
        vec![one]
    };
    let tallest = columns.iter().map(Vec::len).max().unwrap_or(0) as u16;
    let width = (COLUMN * columns.len() as u16).min(full.width);
    let height = (tallest + 2).min(full.height);
    let area = Rect::new(
        full.x + (full.width - width) / 2,
        full.y + (full.height - height) / 2,
        width,
        height,
    );
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .padding(ratatui::widgets::Padding::horizontal(1))
        .title(Span::styled(
            " Keys ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(block, area);
    let parts = Layout::horizontal(vec![Constraint::Fill(1); columns.len()]).split(inner);
    for (lines, part) in columns.into_iter().zip(parts.iter()) {
        frame.render_widget(Paragraph::new(lines), *part);
    }
    app.hits.push((full, Hit::Help));
}

/// The list's title: "Merge requests" (or "Pull requests") for open ones,
/// "Merged merge requests" or "Closed pull requests" for the others.
pub fn list_title(app: &App) -> String {
    let noun = app.plural_noun();
    match app.status {
        Status::Open => noun.to_string(),
        Status::Merged => format!("Merged {}", noun.to_lowercase()),
        Status::Closed => format!("Closed {}", noun.to_lowercase()),
    }
}

fn draw_header(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    let line = Rect::new(area.x + 1, area.y, area.width.saturating_sub(2), 1);
    let icon = icons
        .status(app.status)
        .unwrap_or_else(|| icons.merge_state(MergeState::Ready))
        .0;
    let labels: Vec<(Section, String)> = Section::ORDER
        .iter()
        .map(|&section| {
            // Still loading for this scope ("All", merged and closed ones;
            // open ones count from 0 as before).
            let loading = app.loading_section(section)
                && (section == Section::All || app.status != Status::Open);
            let count = if loading {
                "…".to_string()
            } else {
                app.rows(section).len().to_string()
            };
            (section, format!(" {} {count} ", section.name()))
        })
        .collect();
    // On a pane too narrow for the tabs next to "Merged merge requests",
    // the title is only "Merged".
    let tabs_width: usize = labels.iter().map(|(_, l)| l.chars().count() + 1).sum();
    let mut title = format!("{icon} {}", list_title(app));
    if app.status != Status::Open
        && Line::from(title.as_str()).width() + 3 + tabs_width > line.width as usize
    {
        let state = app.status.name();
        title = format!("{icon} {}{}", state[..1].to_uppercase(), &state[1..]);
    }
    // A click on the title steps the state, like its key.
    let title_width = Line::from(title.as_str()).width() as u16;
    app.hits
        .push((Rect::new(line.x, line.y, title_width, 1), Hit::Status));
    let mut spans = vec![Span::styled(
        format!("{title}   "),
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    let mut x = line.x + Line::from(spans.clone()).width() as u16;
    for (section, label) in labels {
        let active = app.section == section;
        let style = if active {
            Style::new()
                .fg(TEXT)
                .bg(SELECTED_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(MUTED)
        };
        let width = label.chars().count() as u16;
        app.hits
            .push((Rect::new(x, line.y, width, 1), Hit::Section(section)));
        x += width + 1;
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    let left = Line::from(spans);
    let used = left.width() as u16;
    frame.render_widget(Paragraph::new(left), line);

    // Right: active filter and sort as chips, the search field, the scope.
    let chip = Style::new().fg(YELLOW).bg(SELECTED_BG);
    let mut parts: Vec<(String, Style, Hit)> = Vec::new();
    if app.show != Show::All {
        parts.push((format!(" {} ✕ ", app.show.name()), chip, Hit::Key('f')));
    }
    if app.sort != SortKey::Updated || !app.descending {
        let arrow = if app.descending { "↓" } else { "↑" };
        parts.push((
            format!(" {arrow} {} ", app.sort.name()),
            chip,
            Hit::Key('s'),
        ));
    }
    let search = if app.typing {
        (format!("⌕ {}▏", app.filter), Style::new().fg(YELLOW))
    } else if app.filter.is_empty() {
        ("⌕ Search".to_string(), Style::new().fg(FAINT))
    } else {
        (format!("⌕ {}", app.filter), Style::new().fg(YELLOW))
    };
    parts.push((search.0, search.1, Hit::Key('/')));
    let scope = match app.scoped_project() {
        Some(p) => format!("{} {}", icons.repo(), p.name()),
        None => format!("{} all projects", icons.repo()),
    };
    parts.push((scope, Style::new().fg(MUTED), Hit::Scope));

    let gap = 3usize;
    let total: usize = parts
        .iter()
        .map(|(t, _, _)| t.chars().count())
        .sum::<usize>()
        + gap * parts.len().saturating_sub(1);
    if total + 2 <= line.width.saturating_sub(used) as usize {
        let mut x = line.x + line.width - total as u16;
        let mut spans = Vec::new();
        for (i, (text, style, hit)) in parts.into_iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" ".repeat(gap)));
                x += gap as u16;
            }
            let w = text.chars().count() as u16;
            app.hits.push((Rect::new(x, line.y, w, 1), hit));
            x += w;
            spans.push(Span::styled(text, style));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).alignment(Alignment::Right),
            line,
        );
    }
}

/// The columns that fit, each with its width: as wide as its widest entry
/// (or heading), with the title taking the rest.
fn columns(app: &App, width: u16, rows: &[&MergeRequest], icons: &Icons) -> Vec<(Col, Constraint)> {
    let mut cols = vec![Col::State];
    if app.config.compact {
        cols.push(Col::Id);
    }
    cols.extend([Col::Title, Col::People]);
    if matches!(app.section, Section::Review | Section::All) {
        cols.push(Col::You);
    }
    cols.extend([
        Col::Pipeline,
        Col::Approvals,
        Col::Threads,
        Col::Changes,
        Col::Created,
        Col::Updated,
    ]);

    let measure = |c: Col| -> u16 {
        let content = rows
            .iter()
            .map(|mr| cell_line(c, mr, app, icons).width())
            .max()
            .unwrap_or(0) as u16;
        let cap = if c == Col::People { 22 } else { u16::MAX };
        content
            .max(heading(c, app, icons).chars().count() as u16)
            .min(cap)
    };
    let mut sized: Vec<(Col, Constraint)> = cols
        .into_iter()
        .filter(|c| *c != Col::Changes || rows.iter().any(|mr| mr.additions.is_some()))
        .map(|c| {
            let constraint = if c == Col::Title {
                Constraint::Fill(1)
            } else {
                Constraint::Length(measure(c))
            };
            (c, constraint)
        })
        .collect();

    // Keep the title readable on narrow panes: the least needed columns go
    // first.
    let spacing = spacing(width);
    for optional in [
        Col::Created,
        Col::Changes,
        Col::People,
        Col::Approvals,
        Col::Pipeline,
    ] {
        let fixed: u16 = sized
            .iter()
            .map(|(_, c)| match c {
                Constraint::Length(n) => n + spacing,
                _ => spacing,
            })
            .sum();
        if width.saturating_sub(fixed) >= 30 {
            break;
        }
        sized.retain(|(c, _)| *c != optional);
    }
    sized
}

fn spacing(width: u16) -> u16 {
    if width >= 80 {
        3
    } else {
        1
    }
}

/// The sort key a column heading stands for.
fn sort_key(c: Col) -> Option<SortKey> {
    match c {
        Col::Id => Some(SortKey::Number),
        Col::Title => Some(SortKey::Title),
        Col::People => Some(SortKey::Author),
        Col::Threads => Some(SortKey::Threads),
        Col::Changes => Some(SortKey::Size),
        Col::Created => Some(SortKey::Created),
        Col::Updated => Some(SortKey::Updated),
        Col::State | Col::You | Col::Pipeline | Col::Approvals => None,
    }
}

fn heading(c: Col, app: &App, _icons: &Icons) -> String {
    let name: String = match c {
        Col::State | Col::Id => String::new(),
        Col::Title => "Merge request".into(),
        Col::People => match app.section {
            Section::Review | Section::All => "Author".into(),
            Section::Mine => "Reviewers".into(),
        },
        Col::You => "Review".into(),
        Col::Pipeline => "CI".into(),
        Col::Approvals => "Approvals".into(),
        Col::Threads => "Threads".into(),
        Col::Changes => "Changes".into(),
        Col::Created => "Opened".into(),
        Col::Updated => "Updated".into(),
    };
    match sort_key(c) {
        // Same arrow as the sort chip: ↓ for newest, most or biggest first.
        Some(k) if k == app.sort => {
            format!("{name} {}", if app.descending { "↓" } else { "↑" })
        }
        _ => name,
    }
}

/// The second line of a merge request: project and number, and where it
/// stands (merge conflict, waiting for approval, …).
fn meta_line<'a>(mr: &'a MergeRequest, logo: Option<&'static str>) -> Line<'a> {
    let (status, color) = status_text(mr);
    let mut spans = Vec::new();
    if let Some(logo) = logo {
        spans.push(Span::styled(format!("{logo} "), Style::new().fg(MUTED)));
    }
    spans.extend([
        Span::styled(
            format!("{} {}", mr.project_name(), mr.reference()),
            Style::new().fg(FAINT),
        ),
        Span::styled("  ·  ", Style::new().fg(FAINT)),
        Span::styled(status, Style::new().fg(color)),
    ]);
    Line::from(spans)
}

/// The title cell of a two-line row: the title in bold white, the meta line
/// under it.
fn two_line_title<'a>(mr: &'a MergeRequest, logo: Option<&'static str>) -> ratatui::text::Text<'a> {
    ratatui::text::Text::from(vec![
        Line::from(Span::styled(
            clean_title(&mr.title),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        )),
        meta_line(mr, logo),
    ])
}

/// The title without GitLab's "Draft:" prefix; the state icon shows it.
fn clean_title(title: &str) -> &str {
    for prefix in ["Draft: ", "Draft:", "[Draft] ", "(Draft) ", "WIP: "] {
        if let Some(rest) = title.strip_prefix(prefix) {
            return rest.trim_start();
        }
    }
    title
}

fn cell_line<'a>(c: Col, mr: &'a MergeRequest, app: &App, icons: &Icons) -> Line<'a> {
    let me = mr.viewer.as_str();
    let dim = |t: &'static str| Line::from(Span::styled(t, Style::new().fg(FAINT)));
    let age = |t: Option<i64>| {
        Line::from(Span::styled(
            t.map(|t| time::age(time::now() - t)).unwrap_or_default(),
            Style::new().fg(MUTED),
        ))
        .alignment(Alignment::Right)
    };
    match c {
        Col::State => {
            let (icon, color) = icons
                .status(mr.status)
                .unwrap_or_else(|| icons.merge_state(mr.merge_state));
            Line::from(Span::styled(icon, Style::new().fg(color)))
        }
        Col::Id => Line::from(Span::styled(mr.reference(), Style::new().fg(FAINT))),
        Col::Title => Line::from(Span::styled(
            clean_title(&mr.title),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        )),
        Col::People => match app.section {
            Section::Review | Section::All => Line::from(Span::styled(
                format!("@{}", mr.author),
                Style::new().fg(MUTED),
            )),
            Section::Mine => {
                if mr.reviewers.is_empty() {
                    return dim("none");
                }
                let mut spans = Vec::new();
                for (i, r) in mr.reviewers.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::raw("  "));
                    }
                    let (icon, color) = icons.review(Some(r.state));
                    spans.push(Span::styled(format!("{icon} "), Style::new().fg(color)));
                    spans.push(Span::styled(r.username.as_str(), Style::new().fg(MUTED)));
                }
                Line::from(spans)
            }
        },
        // Review and pipeline as a coloured dot only, so the title keeps
        // the room; the details panel says it in words.
        Col::You => {
            // In "All" only where you review: a grey dot would claim a
            // review nobody asked of you.
            if app.section == Section::All && !app.reviews(mr) {
                return Line::from("");
            }
            let color = match mr.review_of(me) {
                Some(ReviewState::Approved) => GREEN,
                Some(ReviewState::ChangesRequested) => RED,
                Some(ReviewState::Reviewed) | Some(ReviewState::Started) => YELLOW,
                _ => FAINT,
            };
            Line::from(Span::styled("●", Style::new().fg(color)))
        }
        Col::Pipeline => {
            let color = match mr.pipeline {
                Pipeline::None => return dim("–"),
                Pipeline::Success => GREEN,
                Pipeline::Failed => RED,
                Pipeline::Running => YELLOW,
                Pipeline::Canceled | Pipeline::Manual => FAINT,
            };
            Line::from(Span::styled("●", Style::new().fg(color)))
        }
        Col::Approvals => {
            // Given of needed (0/2) when the forge says how many are needed.
            let count = match mr.approvals_required {
                Some(n) if n > 0 => format!("{}/{n}", mr.approvals_given),
                _ => mr.approvals_given.to_string(),
            };
            if mr.approved {
                Line::from(Span::styled(
                    format!("{} {count}", icons.approvals()),
                    Style::new().fg(GREEN),
                ))
            } else if mr.approvals_required.unwrap_or(0) > 0 || mr.approvals_given > 0 {
                Line::from(Span::styled(count, Style::new().fg(YELLOW)))
            } else {
                Line::from(Span::styled(count, Style::new().fg(FAINT)))
            }
        }
        Col::Threads => {
            if mr.threads_open > 0 {
                Line::from(Span::styled(
                    format!("{} {} open", icons.threads(), mr.threads_open),
                    Style::new().fg(YELLOW),
                ))
            } else if mr.threads_total > 0 {
                Line::from(Span::styled(
                    format!("{} {} resolved", icons.threads(), mr.threads_total),
                    Style::new().fg(MUTED),
                ))
            } else {
                dim("–")
            }
        }
        Col::Changes => match (mr.additions, mr.deletions) {
            (Some(a), Some(d)) => Line::from(vec![
                Span::styled(format!("+{a}"), Style::new().fg(GREEN)),
                Span::styled(format!(" -{d}"), Style::new().fg(RED)),
            ]),
            _ => dim("–"),
        },
        Col::Created => age(mr.created),
        Col::Updated => age(mr.updated),
    }
}

fn draw_table(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    // No box around the list: a heading with a line under it, and faint
    // lines between the rows, like a list in an app.
    let inner = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );

    let rows_data = app.rows(app.section);
    let mixed = app.mixed_forges();
    let all_failed = match app.all_list() {
        Some(all) if app.section == Section::All && all.items.is_empty() => all.error.clone(),
        _ => None,
    };
    // "All" of a scope (and merged and closed lists) load after `a`, while
    // open ones in the other sections switch at once.
    let section_loading = app.loading_section(app.section);
    if rows_data.is_empty() {
        let (text, color) = if app.data.is_none() || section_loading {
            match &app.error {
                // Nothing loads any more: say why instead of "Loading".
                Some(e) if app.data.is_none() || !app.loading => (e.clone(), RED),
                _ => (
                    format!("Loading {}…", list_title(app).to_lowercase()),
                    MUTED,
                ),
            }
        } else if let Some(e) = all_failed {
            (e, RED)
        } else {
            let place = match (app.scoped_project(), app.section) {
                (Some(p), _) => format!(" in {}", p.name()),
                (None, Section::All) => " in your projects".to_string(),
                (None, _) => String::new(),
            };
            let hint = if app.scoped_project().is_some() {
                "  (a shows all projects)"
            } else {
                ""
            };
            let noun = app.plural_noun().to_lowercase();
            let state = app.status.name();
            let what = match (app.section, app.status) {
                (Section::Review, Status::Open) => "Nothing to review",
                (Section::Review, _) => &format!("No {state} {noun} with you as reviewer"),
                (Section::Mine, _) => &format!("No {state} {noun} of yours"),
                (Section::All, _) => &format!("No {state} {noun}"),
            };
            (format!("{what}{place}{hint}"), MUTED)
        };
        // Wrapped on narrow panes instead of cut off.
        let lines = text::wrap(
            vec![Span::styled(text, Style::new().fg(color))],
            inner.width,
            Span::raw(""),
            Span::raw(""),
        );
        let [_, mid, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(lines.len().max(1) as u16),
            Constraint::Fill(1),
        ])
        .areas(inner);
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), mid);
        return;
    }

    let sized = columns(app, inner.width, &rows_data, icons);
    let cols: Vec<Col> = sized.iter().map(|(c, _)| *c).collect();
    let widths: Vec<Constraint> = sized.iter().map(|(_, w)| *w).collect();
    let header = Row::new(
        cols.iter()
            .map(|c| Cell::from(heading(*c, app, icons)))
            .collect::<Vec<_>>(),
    )
    // The headings on a bar of their own, like the header of a table in an
    // app.
    .style(
        Style::new()
            .fg(MUTED)
            .bg(BAR_BG)
            .add_modifier(Modifier::BOLD),
    );
    // Clickable headings: the same layout the table uses, after the
    // one-cell selection bar.
    let heading_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let rects = Layout::horizontal(widths.clone())
        .flex(Flex::Legacy)
        .spacing(spacing(inner.width))
        .split(heading_area);
    let heading_hits: Vec<(Rect, Hit)> = cols
        .iter()
        .zip(rects.iter())
        .filter_map(|(c, r)| sort_key(*c).map(|k| (*r, Hit::Sort(k))))
        .collect();
    // The selected row gets its colour as the row style, under the cells, so
    // labels keep their own colour across the whole pill.
    let selected = app.table_state(app.section).selected();
    // Rows follow each other directly; the dim first line of each
    // merge request marks where it starts.
    let gap: u16 = 0;
    let height: u16 = if app.config.compact { 1 } else { 2 };
    let rows: Vec<Row> = rows_data
        .iter()
        .enumerate()
        .map(|(i, mr)| {
            let row = Row::new(
                cols.iter()
                    .map(|c| {
                        if *c == Col::Title && height == 2 {
                            Cell::from(two_line_title(
                                mr,
                                if mixed { icons.forge(mr.forge) } else { None },
                            ))
                        } else {
                            Cell::from(cell_line(*c, mr, app, icons))
                        }
                    })
                    .collect::<Vec<_>>(),
            )
            .height(height)
            .bottom_margin(gap);
            if selected == Some(i) {
                row.style(Style::new().bg(SELECTED_BG))
            } else {
                row
            }
        })
        .collect();
    let row_count = rows.len();
    let header_rows = 1;
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(spacing(inner.width))
        .highlight_spacing(HighlightSpacing::Never);
    // Rows borrow the app, so render with a copy of the state and keep the
    // scroll offset it settles on.
    let mut state = *app.table_state(app.section);
    frame.render_stateful_widget(table, inner, &mut state);
    *app.state_mut() = state;

    // The line under the headings, and between the visible rows.
    app.hits.extend(heading_hits);
    // Each visible row can be clicked.
    let first_y = inner.y + header_rows;
    let step = height + gap;
    let bottom = inner.y + inner.height;
    let visible = (inner.height.saturating_sub(header_rows) as usize).div_ceil(step as usize);
    for (n, index) in (state.offset()..row_count).take(visible).enumerate() {
        let y = first_y + n as u16 * step;
        if y >= bottom {
            break;
        }
        let h = height.min(bottom - y);
        app.hits
            .push((Rect::new(inner.x, y, inner.width, h), Hit::Row(index)));
    }
}

fn status_text(mr: &MergeRequest) -> (&'static str, ratatui::style::Color) {
    // What blocks a merge means nothing once it is merged or closed.
    match mr.status {
        Status::Merged => return ("Merged", ACCENT),
        Status::Closed => return ("Closed", RED),
        Status::Open => {}
    }
    match mr.merge_state {
        MergeState::Ready => ("Ready to merge", ACCENT),
        MergeState::Draft => ("Draft", MUTED),
        MergeState::Conflict => ("Merge conflict", RED),
        MergeState::NeedsRebase => ("Needs rebase", YELLOW),
        MergeState::Checking => ("Checking", YELLOW),
        MergeState::Blocked => {
            if mr.pipeline == Pipeline::Failed {
                ("Pipeline failed", RED)
            } else if mr.threads_open > 0 {
                ("Open threads", YELLOW)
            } else if !mr.approved {
                ("Waiting for approval", YELLOW)
            } else {
                ("Blocked", MUTED)
            }
        }
    }
}

fn review_word(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Unreviewed => "not reviewed yet",
        ReviewState::Started => "reviewing",
        ReviewState::Reviewed => "commented",
        ReviewState::ChangesRequested => "requested changes",
        ReviewState::Approved => "approved",
    }
}

fn pipeline_word(p: Pipeline) -> &'static str {
    match p {
        Pipeline::None => "no pipeline",
        Pipeline::Success => "passed",
        Pipeline::Failed => "failed",
        Pipeline::Running => "running",
        Pipeline::Canceled => "cancelled or skipped",
        Pipeline::Manual => "waiting for a manual job",
    }
}

/// The description without HTML comments (merge request templates are full
/// of them).
fn plain_description(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    let mut lines: Vec<&str> = Vec::new();
    for line in out.lines().map(str::trim_end) {
        if line.is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

fn draw_details(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    let Some(mr) = app.selected().cloned() else {
        return;
    };
    let mr = &mr;
    let me = mr.viewer.clone();
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .padding(ratatui::widgets::Padding::horizontal(1))
        .title(panel_title(app, mr, area));
    let inner = block.inner(area);
    let width = inner.width;
    const LABEL: usize = 12;
    let pad = || Span::raw(" ".repeat(LABEL));
    // One field: a dim label, the value wrapped with a hanging indent.
    let field = |name: &str, value: Vec<Span<'static>>| -> Vec<Line<'static>> {
        text::wrap(
            value,
            width,
            Span::styled(format!("{name:<LABEL$}"), Style::new().fg(FAINT)),
            Span::raw(" ".repeat(LABEL)),
        )
    };
    let (status, color) = status_text(mr);

    if app.panel_tab == PanelTab::Activity {
        let (lines, spots, selected_line, urls) = activity_lines(
            app.selected_activity(),
            &me,
            width,
            app.thread_filter,
            app.thread,
            mr.status == Status::Open,
        );
        // Bring a thread chosen with n or N into view.
        if app.scroll_to_thread {
            app.scroll_to_thread = false;
            if let Some(line) = selected_line {
                let line = line as u16;
                if line < app.details_scroll || line >= app.details_scroll + inner.height {
                    app.details_scroll = line.saturating_sub(1);
                }
            }
        }
        let max_scroll = (lines.len() as u16).saturating_sub(inner.height / 2);
        app.details_scroll = app.details_scroll.min(max_scroll);
        frame.render_widget(
            Paragraph::new(lines)
                .block(block)
                .scroll((app.details_scroll, 0)),
            area,
        );
        app.hits.push((area, Hit::Details));
        // Clickable chips, threads, buttons and links, where they are on
        // screen.
        let base = app.links.len();
        app.links.extend(urls);
        let spots = spots.into_iter().map(|(l, x, w, h)| match h {
            Hit::Link(i) => (l, x, w, Hit::Link(base + i)),
            other => (l, x, w, other),
        });
        for (line, x, w, hit) in spots {
            let line = line as u16;
            if line < app.details_scroll || line - app.details_scroll >= inner.height {
                continue;
            }
            let y = inner.y + line - app.details_scroll;
            let x = inner.x + x.min(inner.width);
            let w = w.min(inner.x + inner.width - x);
            app.hits.push((Rect::new(x, y, w, 1), hit));
        }
        push_panel_tab_hits(app, mr, area);
        return;
    }

    let mut lines: Vec<Line<'static>> = text::wrap(
        vec![Span::styled(
            clean_title(&mr.title).to_string(),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        )],
        width,
        Span::raw(""),
        Span::raw(""),
    );
    lines.push(Line::from(""));
    lines.extend(field(
        "Status",
        vec![Span::styled(
            format!(" {status} "),
            Style::new()
                .fg(theme::on(color))
                .bg(color)
                .add_modifier(Modifier::BOLD),
        )],
    ));
    lines.extend(field(
        "Branch",
        vec![
            Span::styled(format!("{} ", mr.source_branch), Style::new().fg(MUTED)),
            Span::styled("→ ", Style::new().fg(FAINT)),
            Span::styled(mr.target_branch.clone(), Style::new().fg(MUTED)),
        ],
    ));
    lines.extend(field(
        "Author",
        vec![
            Span::styled(format!("{} ", mr.author), Style::new().fg(TEXT)),
            Span::styled(
                mr.created
                    .map(|t| format!(" opened {} ago", time::age(time::now() - t)))
                    .unwrap_or_default(),
                Style::new().fg(FAINT),
            ),
        ],
    ));
    if mr.reviewers.is_empty() {
        lines.extend(field(
            "Reviewers",
            vec![Span::styled("none", Style::new().fg(FAINT))],
        ));
    }
    for (i, r) in mr.reviewers.iter().enumerate() {
        let (icon, c) = icons.review(Some(r.state));
        let value = vec![
            Span::styled(format!("{icon} "), Style::new().fg(c)),
            Span::styled(format!("{} ", r.username), Style::new().fg(TEXT)),
            Span::styled(format!(" {}", review_word(r.state)), Style::new().fg(FAINT)),
        ];
        if i == 0 {
            lines.extend(field("Reviewers", value));
        } else {
            lines.extend(text::wrap(value, width, pad(), pad()));
        }
    }
    let approvals = if mr.approved || (mr.status != Status::Open && !mr.approved_by.is_empty()) {
        Span::styled(
            format!("approved by {}", mr.approved_by.join(", ")),
            Style::new().fg(GREEN),
        )
    } else if mr.status != Status::Open {
        // Nothing waits any more.
        Span::styled("none", Style::new().fg(FAINT))
    } else if mr.pending_rules.is_empty() {
        Span::styled("not approved yet", Style::new().fg(YELLOW))
    } else {
        Span::styled(
            format!("waiting for {}", mr.pending_rules.join(", ")),
            Style::new().fg(YELLOW),
        )
    };
    lines.extend(field("Approvals", vec![approvals]));
    let threads = if mr.threads_total == 0 {
        Span::styled("none", Style::new().fg(FAINT))
    } else if mr.threads_open == 0 {
        Span::styled(
            format!("all {} resolved", mr.threads_total),
            Style::new().fg(GREEN),
        )
    } else {
        Span::styled(
            format!("{} of {} open", mr.threads_open, mr.threads_total),
            Style::new().fg(YELLOW),
        )
    };
    lines.extend(field("Threads", vec![threads]));
    let (picon, pcolor) = icons.pipeline(mr.pipeline);
    lines.extend(field(
        "Pipeline",
        vec![Span::styled(
            format!("{picon} {}", pipeline_word(mr.pipeline)),
            Style::new().fg(pcolor),
        )],
    ));
    if let (Some(a), Some(d)) = (mr.additions, mr.deletions) {
        lines.extend(field(
            "Changes",
            vec![
                Span::styled(format!("+{a} "), Style::new().fg(GREEN)),
                Span::styled(format!("-{d}"), Style::new().fg(RED)),
            ],
        ));
    }
    if !mr.labels.is_empty() {
        let mut spans = Vec::new();
        for l in &mr.labels {
            spans.extend(label_pill(l, icons).into_iter().map(owned));
            spans.push(Span::raw("  "));
        }
        lines.extend(field("Labels", spans));
    }
    // Clickable links in the overview: (line, column, width, URL).
    let mut link_spots: Vec<text::LinkSpot> = Vec::new();
    {
        let (l, spots) = text::wrap_tagged(
            vec![(
                Span::styled(
                    mr.url.clone(),
                    Style::new().fg(BLUE).add_modifier(Modifier::UNDERLINED),
                ),
                Some(mr.url.clone()),
            )],
            width,
            Span::styled(format!("{:<LABEL$}", "Link"), Style::new().fg(FAINT)),
            Span::raw(" ".repeat(LABEL)),
        );
        let base = lines.len();
        link_spots.extend(spots.into_iter().map(|(i, x, w, u)| (i + base, x, w, u)));
        lines.extend(l);
    }

    // What can be done from here.
    let key = Style::new().fg(MUTED).add_modifier(Modifier::BOLD);
    let mut actions = vec![
        Span::styled("⏎", key),
        Span::styled(" open   ", Style::new().fg(FAINT)),
        Span::styled("d", key),
        Span::styled(" changes   ", Style::new().fg(FAINT)),
    ];
    // Merged and closed ones can only be looked at.
    let open = mr.status == Status::Open;
    if open && app.herdr && app.remote_of(mr).is_some() {
        actions.push(Span::styled("b", key));
        actions.push(Span::styled(" branch   ", Style::new().fg(FAINT)));
    }
    if open && mr.author != me {
        let what = if mr.approved_by.contains(&me) {
            " revoke approval   "
        } else {
            " approve   "
        };
        actions.push(Span::styled("v", key));
        actions.push(Span::styled(what, Style::new().fg(FAINT)));
    }
    if open && mr.merge_state == MergeState::Ready {
        actions.push(Span::styled("m", key));
        actions.push(Span::styled(" merge   ", Style::new().fg(FAINT)));
    }
    actions.push(Span::styled("c", key));
    actions.push(Span::styled(" activity", Style::new().fg(FAINT)));
    lines.push(Line::from(""));
    lines.extend(text::wrap(actions, width, Span::raw(""), Span::raw("")));

    // The description, with its Markdown rendered.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::new().fg(BORDER),
    )));
    let description = plain_description(&mr.description);
    if description.is_empty() {
        lines.push(Line::from(Span::styled(
            "No description.",
            Style::new().fg(FAINT),
        )));
    } else {
        let (l, spots) = text::markdown_links(&description, width);
        let base = lines.len();
        link_spots.extend(spots.into_iter().map(|(i, x, w, u)| (i + base, x, w, u)));
        lines.extend(l);
    }

    // Keep the scroll inside the text.
    let max_scroll = (lines.len() as u16).saturating_sub(inner.height / 2);
    app.details_scroll = app.details_scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.details_scroll, 0)),
        area,
    );
    app.hits.push((area, Hit::Details));
    for (line, x, w, url) in link_spots {
        let line = line as u16;
        if line < app.details_scroll || line - app.details_scroll >= inner.height {
            continue;
        }
        let y = inner.y + line - app.details_scroll;
        let x = inner.x + x.min(inner.width);
        let w = w.min(inner.x + inner.width - x);
        app.hits
            .push((Rect::new(x, y, w, 1), Hit::Link(app.links.len())));
        app.links.push(url);
    }
    push_panel_tab_hits(app, mr, area);
}

/// The panel's title: number, project and the two tabs.
fn panel_title(app: &App, mr: &MergeRequest, _area: Rect) -> Line<'static> {
    let tab = |name: &'static str, t: PanelTab| {
        if app.panel_tab == t {
            Span::styled(
                format!(" {name} "),
                Style::new()
                    .fg(theme::on(ACCENT))
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" {name} "), Style::new().fg(MUTED))
        }
    };
    Line::from(vec![
        Span::styled(
            format!(" {} · {} ", mr.reference(), mr.project_name()),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        tab("Overview", PanelTab::Overview),
        Span::raw(" "),
        tab("Activity", PanelTab::Activity),
        Span::raw(" "),
    ])
}

/// Clickable tabs in the panel's title.
fn push_panel_tab_hits(app: &mut App, mr: &MergeRequest, area: Rect) {
    // Title starts one cell after the corner.
    let head = format!(" {} · {} ", mr.reference(), mr.project_name())
        .chars()
        .count() as u16;
    let x = area.x + 1 + head + 1;
    app.hits
        .push((Rect::new(x, area.y, 10, 1), Hit::Panel(PanelTab::Overview)));
    app.hits.push((
        Rect::new(x + 11, area.y, 10, 1),
        Hit::Panel(PanelTab::Activity),
    ));
}

/// A clickable place in the activity text: line index, column, width, what.
type ActivityHit = (usize, u16, u16, Hit);

/// The activity tab: a filter line (all, open, resolved), then the
/// discussions, newest first, with their replies. Threads can be selected
/// and resolved; the clickable places come back with the lines. Replies
/// and comments are offered when `writable` (open ones).
fn activity_lines(
    activity: Option<&Activity>,
    me: &str,
    width: u16,
    filter: ThreadFilter,
    selected: Option<usize>,
    writable: bool,
) -> (
    Vec<Line<'static>>,
    Vec<ActivityHit>,
    Option<usize>,
    Vec<String>,
) {
    let mut hits: Vec<ActivityHit> = Vec::new();
    // Link targets; `Hit::Link` indexes into this list.
    let mut urls: Vec<String> = Vec::new();
    let list = match activity {
        None | Some(Activity::Loading) => {
            return (
                vec![Line::from(Span::styled(
                    "Loading comments…",
                    Style::new().fg(MUTED),
                ))],
                hits,
                None,
                urls,
            )
        }
        Some(Activity::Failed(e)) => {
            return (
                vec![Line::from(Span::styled(e.clone(), Style::new().fg(RED)))],
                hits,
                None,
                urls,
            )
        }
        Some(Activity::Loaded(list)) => list,
    };

    // Filter chips with counts.
    let open = list.iter().filter(|d| d.resolvable && !d.resolved).count();
    let resolved = list.iter().filter(|d| d.resolvable && d.resolved).count();
    let mut chips = Vec::new();
    let mut x = 0u16;
    for (f, name) in [
        (ThreadFilter::All, "All".to_string()),
        (ThreadFilter::Open, format!("Open {open}")),
        (ThreadFilter::Resolved, format!("Resolved {resolved}")),
    ] {
        let text = format!(" {name} ");
        let style = if f == filter {
            Style::new()
                .fg(TEXT)
                .bg(SELECTED_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(MUTED)
        };
        let w = text.chars().count() as u16;
        hits.push((0, x, w, Hit::Threads(f)));
        x += w + 1;
        chips.push(Span::styled(text, style));
        chips.push(Span::raw(" "));
    }
    let mut lines: Vec<Line<'static>> = vec![Line::from(chips)];
    // What the keys do, spelled out: the selected thread has a bar on the left.
    let hint = |key: &'static str, what: &'static str| {
        [
            Span::styled(key, Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(what, Style::new().fg(FAINT)),
        ]
    };
    let mut keys = Vec::new();
    let mut hints = vec![
        ("t", " filter   "),
        ("n", " next thread   "),
        ("N", " previous   "),
        ("e", " resolve / reopen   "),
    ];
    if writable {
        hints.extend([(">", " reply   "), ("+", " comment")]);
    }
    for (k, w) in hints {
        keys.extend(hint(k, w));
    }
    lines.extend(text::wrap(keys, width, Span::raw(""), Span::raw("")));
    lines.push(Line::from(""));
    // Where the discussions start; each one but the first gets a blank
    // line before it.
    let top = lines.len() - 1;
    let mut selected_line = None;

    let shown: Vec<(usize, &crate::model::Discussion)> = list
        .iter()
        .enumerate()
        .filter(|(_, d)| filter.keeps(d))
        .collect();
    if shown.is_empty() {
        let what = match filter {
            ThreadFilter::All => "No activity yet.",
            ThreadFilter::Open => "No open threads.",
            ThreadFilter::Resolved => "No resolved threads.",
        };
        lines.push(Line::from(Span::styled(what, Style::new().fg(FAINT))));
        return (lines, hits, None, urls);
    }

    let age = |t: Option<i64>| {
        t.map(|t| format!("{} ago", time::age(time::now() - t)))
            .unwrap_or_default()
    };
    let mut last_was_system = false;
    for (index, d) in shown {
        // GitLab's own notes: one dim line each, grouped.
        if d.notes.len() == 1 && d.notes[0].system {
            if lines.len() > top && !last_was_system {
                lines.push(Line::from(""));
            }
            last_was_system = true;
            let n = &d.notes[0];
            let body = n.body.lines().next().unwrap_or("").to_string();
            lines.extend(text::wrap(
                vec![
                    Span::styled(format!("{} ", n.author), Style::new().fg(MUTED)),
                    Span::styled(format!("{body}  "), Style::new().fg(FAINT)),
                    Span::styled(age(n.created), Style::new().fg(FAINT)),
                ],
                width,
                Span::styled("· ", Style::new().fg(FAINT)),
                Span::raw("  "),
            ));
            continue;
        }
        last_was_system = false;
        if lines.len() > top {
            lines.push(Line::from(""));
        }
        let is_selected = selected == Some(index);
        let bar = if is_selected {
            Span::styled("▌", Style::new().fg(ACCENT))
        } else {
            Span::raw(" ")
        };

        // Thread header: where it is, whether it is resolved, a button.
        if d.resolvable {
            let start = lines.len();
            if is_selected {
                selected_line = Some(start);
            }
            let mut head = vec![bar.clone(), Span::raw(" ")];
            let mut used = 2u16;
            let (state, color) = if d.resolved {
                ("✓ resolved", GREEN)
            } else {
                ("● open", YELLOW)
            };
            head.push(Span::styled(state, Style::new().fg(color)));
            used += state.chars().count() as u16;
            let button = if d.resolved { " Reopen " } else { " Resolve " };
            let bw = button.chars().count() as u16;
            // The file takes what is left between the state and the button.
            if let Some(place) = &d.place {
                let room = width.saturating_sub(used + 3 + bw + 2) as usize;
                if room >= 6 {
                    let place = cut(place, room);
                    used += 3 + place.chars().count() as u16;
                    head.push(Span::styled(format!("   {place}"), Style::new().fg(BLUE)));
                }
            }
            let pad = width.saturating_sub(used + bw).max(2);
            head.push(Span::raw(" ".repeat(pad as usize)));
            head.push(Span::styled(
                button,
                if d.resolved {
                    Style::new().fg(TEXT).bg(BORDER)
                } else {
                    Style::new()
                        .fg(theme::on(GREEN))
                        .bg(GREEN)
                        .add_modifier(Modifier::BOLD)
                },
            ));
            hits.push((start, 0, used, Hit::Thread(index)));
            hits.push((start, used + pad, bw, Hit::Resolve(index)));
            let style = if is_selected {
                Style::new().bg(SELECTED_BG)
            } else {
                Style::new()
            };
            lines.push(Line::from(head).style(style));
        }
        for (i, n) in d.notes.iter().enumerate() {
            let name_style = if n.author == me {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
            };
            let start = lines.len();
            lines.push(Line::from(vec![
                bar.clone(),
                Span::raw(" "),
                Span::styled(
                    if i == 0 {
                        String::new()
                    } else {
                        "↳ ".to_string()
                    },
                    Style::new().fg(FAINT),
                ),
                Span::styled(n.author.clone(), name_style),
                Span::styled(format!("  {}", age(n.created)), Style::new().fg(FAINT)),
            ]));
            if d.resolvable {
                hits.push((start, 0, width, Hit::Thread(index)));
            }
            let pad = if i == 0 { "  " } else { "    " };
            let body_width = width.saturating_sub(pad.len() as u16 + 1);
            let (body, links) = text::markdown_links(&plain_description(&n.body), body_width);
            let base = lines.len();
            let shift = 1 + pad.len() as u16;
            for (l, x, w, url) in links {
                hits.push((base + l, x + shift, w, Hit::Link(urls.len())));
                urls.push(url);
            }
            for l in body {
                let mut spans = vec![bar.clone(), Span::raw(pad.to_string())];
                spans.extend(l.spans);
                lines.push(Line::from(spans));
            }
        }
    }
    (lines, hits, selected_line, urls)
}

/// A span that owns its text, for lines that outlive the merge request.
fn owned(span: Span<'_>) -> Span<'static> {
    Span::styled(span.content.into_owned(), span.style)
}

fn draw_footer(frame: &mut Frame, app: &mut App, icons: &Icons, area: Rect) {
    // A status bar across the whole width.
    frame.render_widget(Block::new().style(Style::new().bg(BAR_BG)), area);
    if app.ask_quit {
        let question = " Close the git panel? ";
        let qw = question.chars().count() as u16;
        let line = Line::from(vec![
            Span::styled(question, Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(
                " y close ",
                Style::new()
                    .fg(theme::on(RED))
                    .bg(RED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(" any other key stays ", Style::new().fg(TEXT).bg(BORDER)),
        ]);
        app.hits
            .push((Rect::new(area.x + qw, area.y, 9, 1), Hit::Key('y')));
        app.hits
            .push((Rect::new(area.x + qw + 10, area.y, 21, 1), Hit::Key('n')));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if let Some(c) = &app.compose {
        draw_compose_footer(frame, app, c.clone(), area);
        return;
    }
    if let Some(c) = &app.confirm {
        let verb = match c.action {
            Action::Approve => "Approve",
            Action::Revoke => "Revoke your approval of",
            Action::Merge => "Merge",
        };
        let question = format!(
            " {verb} {} {}? ",
            c.mr.reference(),
            cut(clean_title(&c.mr.title), 50)
        );
        let yes = " y yes ";
        let no = " n no ";
        let qw = question.chars().count() as u16;
        let line = Line::from(vec![
            Span::styled(question, Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(
                yes,
                Style::new()
                    .fg(theme::on(GREEN))
                    .bg(GREEN)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(no, Style::new().fg(TEXT).bg(BORDER)),
        ]);
        app.hits
            .push((Rect::new(area.x + qw, area.y, 7, 1), Hit::Key('y')));
        app.hits
            .push((Rect::new(area.x + qw + 8, area.y, 6, 1), Hit::Key('n')));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let right = if let Some((text, _)) = &app.flash {
        Span::styled(format!("{text} "), Style::new().fg(ACCENT))
    } else if app.loading {
        Span::styled(
            format!("{} refreshing… ", icons.refresh()),
            Style::new().fg(YELLOW),
        )
    } else if let (Some(e), true) = (&app.error, app.data.is_some()) {
        Span::styled(format!("{e} "), Style::new().fg(RED))
    } else if let Some(at) = app.updated_at {
        Span::styled(
            format!(
                "{} updated {} ago ",
                icons.refresh(),
                short_age(at.elapsed().as_secs())
            ),
            Style::new().fg(FAINT),
        )
    } else {
        Span::raw("")
    };
    // `r` refreshes now; shown (and clickable) in front of the status.
    let refresh_key = if app.loading || app.flash.is_some() {
        None
    } else {
        Some(vec![
            Span::styled("r", Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(" refresh  ", Style::new().fg(FAINT)),
        ])
    };
    let key_width: u16 = refresh_key
        .as_ref()
        .map(|k| k.iter().map(|s| s.width() as u16).sum())
        .unwrap_or(0);
    // "All" cut at what one request per forge loads: say so, in front,
    // when it fits next to the status.
    let cut = app
        .all_list()
        .filter(|all| app.section == Section::All && all.total > all.newest as u64)
        .map(|all| {
            Span::styled(
                format!("newest {} of {}  ", all.newest, all.total),
                Style::new().fg(MUTED),
            )
        })
        .filter(|c| c.width() + right.width() + key_width as usize <= area.width as usize);
    let cut_width = cut.as_ref().map(|c| c.width() as u16).unwrap_or(0);
    // The status on the right wins; the key hints take what is left.
    let right_width = (right.width() as u16 + key_width + cut_width).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(area);
    if key_width > 0 {
        app.hits.push((
            Rect::new(
                right_area.x + cut_width,
                area.y,
                key_width.saturating_sub(2),
                1,
            ),
            Hit::Key('r'),
        ));
    }
    let mut right_spans: Vec<Span> = cut.into_iter().collect();
    right_spans.extend(refresh_key.unwrap_or_default());
    right_spans.push(right);
    let right = Line::from(right_spans);
    // Key hints; the ones with a letter can be clicked.
    // Only the most used keys; ? shows all of them.
    let keys: [(&str, &str, Option<char>); 5] = [
        ("⏎", "open", Some('o')),
        ("p", "details", Some('p')),
        ("/", "search", Some('/')),
        ("?", "all keys", Some('?')),
        ("q", "close", Some('q')),
    ];
    let mut spans = vec![Span::raw(" ")];
    let mut x = left_area.x + 1;
    for (key, what, click) in keys {
        let text = format!(" {what}  ");
        let width = (key.chars().count() + text.chars().count()) as u16;
        if let Some(c) = click {
            if x + width <= left_area.x + left_area.width {
                app.hits
                    .push((Rect::new(x, area.y, width - 2, 1), Hit::Key(c)));
            }
        }
        x += width;
        spans.push(Span::styled(
            key,
            Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(text, Style::new().fg(FAINT)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), left_area);

    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        right_area,
    );
}

/// The footer while a reply or comment is written: its keys, and in the
/// preview the question with clickable buttons (only `y` posts).
fn draw_compose_footer(frame: &mut Frame, app: &mut App, c: Compose, area: Rect) {
    let bold = Style::new().fg(TEXT).add_modifier(Modifier::BOLD);
    let button = |text: &'static str, bg| {
        Span::styled(
            text,
            Style::new()
                .fg(theme::on(bg))
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )
    };
    let plain = |text: &'static str| Span::styled(text, Style::new().fg(TEXT).bg(BORDER));
    // Buttons first in the room left after the text, each with its key.
    let (head, buttons): (String, Vec<(Span<'static>, Option<char>)>) = match c.stage {
        Stage::Writing => (
            format!(" {}  ", cut(&c.title(), 60)),
            vec![
                (button(" Ctrl+s send ", GREEN), Some('s')),
                (Span::raw(" "), None),
                (plain(" Esc discard "), Some('\u{1b}')),
                (Span::styled("   ⏎ new line ", Style::new().fg(FAINT)), None),
            ],
        ),
        Stage::Preview => (
            format!(" {} ", cut(&c.question(), 70)),
            vec![
                (button(" y post ", GREEN), Some('y')),
                (Span::raw(" "), None),
                (plain(" n edit "), Some('n')),
                (Span::raw(" "), None),
                (plain(" Esc discard "), Some('\u{1b}')),
            ],
        ),
        Stage::Posting => (
            format!(" Posting the {} to {}… ", c.noun(), c.mr.reference()),
            Vec::new(),
        ),
    };
    // A narrow footer shortens the text, never the buttons.
    let buttons_width: usize = buttons.iter().map(|(s, _)| s.width()).sum();
    let head = cut(
        &head,
        (area.width as usize).saturating_sub(buttons_width).max(4),
    );
    let mut x = area.x + head.chars().count() as u16;
    let mut spans = vec![Span::styled(head, bold)];
    for (span, key) in buttons {
        let w = span.width() as u16;
        if let Some(k) = key {
            if x + w <= area.x + area.width {
                app.hits.push((Rect::new(x, area.y, w, 1), Hit::Compose(k)));
            }
        }
        x += w;
        spans.push(span);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A label as GitLab shows it: its colour as background, with rounded ends
/// when the font has the Powerline half circles (Nerd Font).
fn label_pill<'a>(l: &'a Label, icons: &Icons) -> Vec<Span<'a>> {
    let (bg, fg) = theme::tint(theme::hex(&l.color).unwrap_or(BORDER));
    let body = Style::new().fg(fg).bg(bg);
    match icons.pill_caps() {
        Some((left, right)) => vec![
            Span::styled(left, Style::new().fg(bg)),
            Span::styled(l.title.as_str(), body),
            Span::styled(right, Style::new().fg(bg)),
        ],
        None => vec![Span::styled(format!(" {} ", l.title), body)],
    }
}

/// At most `max` characters, with an ellipsis when cut.
fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `12s`, then the usual short ages.
fn short_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        time::age(secs as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_drops_comments_and_blank_runs() {
        let d = "<!-- template -->\n## Why\n\n\n\nBecause.<!-- x\ny -->\n";
        assert_eq!(plain_description(d), "## Why\n\nBecause.");
    }

    #[test]
    fn draft_prefix_is_removed() {
        assert_eq!(clean_title("Draft: Add export"), "Add export");
        assert_eq!(clean_title("Add export"), "Add export");
        assert_eq!(cut("abcdef", 4), "abc…");
        assert_eq!(cut("abc", 4), "abc");
    }

    use crate::config::Config;
    use crate::model::{AllList, Data, Forge, Reviewer};
    use crate::repo::Project;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use ratatui::Terminal;

    /// A made-up merge request; `me` gave the review `review`.
    fn sample_mr(iid: u64, review: ReviewState, pipeline: Pipeline) -> MergeRequest {
        MergeRequest {
            forge: Forge::GitLab,
            host: "gitlab.com".into(),
            viewer: "me".into(),
            iid,
            title: "Add export".into(),
            description: String::new(),
            url: String::new(),
            project: "acme/app".into(),
            author: "alex".into(),
            source_branch: "add-export".into(),
            from_fork: false,
            head_sha: "abc123".into(),
            target_branch: "main".into(),
            created: Some(0),
            updated: Some(iid as i64),
            merge_state: MergeState::Blocked,
            status: crate::model::Status::Open,
            approvals_given: 0,
            approvals_required: None,
            approved_by: vec![],
            approved: false,
            pending_rules: vec![],
            reviewers: vec![Reviewer {
                username: "me".into(),
                state: review,
            }],
            threads_total: 0,
            threads_open: 0,
            labels: vec![],
            pipeline,
            additions: Some(3),
            deletions: Some(1),
            merge_method: String::new(),
        }
    }

    /// Draws the whole list into a test terminal.
    fn render(app: &mut App, width: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Where `word` starts in row `y`, as a whole word.
    fn word_x(buf: &Buffer, y: u16, word: &str) -> Option<u16> {
        let cells: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        let letters: Vec<String> = word.chars().map(String::from).collect();
        (0..=cells.len().checked_sub(letters.len())?).find_map(|x| {
            let fits = letters.iter().zip(&cells[x..]).all(|(l, c)| l == c);
            // A whole word: blank (or the edge) before and after.
            let before = x == 0 || cells[x - 1] == " ";
            let after = cells.get(x + letters.len()).is_none_or(|c| *c == " ");
            (fits && before && after).then_some(x as u16)
        })
    }

    /// The heading bar: the row on the bar colour that says "Updated" (a
    /// column narrow panes keep), and where "Review" and "CI" start in it.
    struct Headings {
        y: u16,
        review: Option<u16>,
        ci: Option<u16>,
    }

    fn headings(buf: &Buffer) -> Headings {
        let y = (0..buf.area.height)
            .find(|&y| word_x(buf, y, "Updated").is_some_and(|x| buf[(x, y)].bg == BAR_BG))
            .expect("heading bar");
        Headings {
            y,
            review: word_x(buf, y, "Review"),
            ci: word_x(buf, y, "CI"),
        }
    }

    /// The symbol and colour at column `x` of terminal row `y`.
    fn cell_at(buf: &Buffer, x: u16, y: u16) -> (String, Color) {
        let cell = &buf[(x, y)];
        (cell.symbol().to_string(), cell.fg)
    }

    fn app_with(review: Vec<MergeRequest>, mine: Vec<MergeRequest>, compact: bool) -> App {
        let config = Config {
            compact,
            icons_chosen: true,
            ..Config::default()
        };
        let mut app = App::new(config, None, None);
        app.data = Some(Data {
            review,
            mine,
            of: None,
            all: None,
        });
        // Not from the shell the tests run in.
        app.herdr = false;
        app
    }

    /// A made-up merge request per review and pipeline state, newest first
    /// (the list's order), with the review dot and the CI cell it shows.
    fn samples() -> Vec<(MergeRequest, Color, (&'static str, Color))> {
        vec![
            (
                sample_mr(6, ReviewState::Reviewed, Pipeline::Canceled),
                YELLOW,
                ("●", FAINT),
            ),
            (
                sample_mr(5, ReviewState::Unreviewed, Pipeline::Manual),
                FAINT,
                ("●", FAINT),
            ),
            (
                sample_mr(4, ReviewState::Approved, Pipeline::Success),
                GREEN,
                ("●", GREEN),
            ),
            (
                sample_mr(3, ReviewState::ChangesRequested, Pipeline::Failed),
                RED,
                ("●", RED),
            ),
            (
                sample_mr(2, ReviewState::Started, Pipeline::Running),
                YELLOW,
                ("●", YELLOW),
            ),
            (
                sample_mr(1, ReviewState::Unreviewed, Pipeline::None),
                FAINT,
                ("–", FAINT),
            ),
        ]
    }

    fn sample_rows() -> Vec<MergeRequest> {
        samples().into_iter().map(|(mr, _, _)| mr).collect()
    }

    /// Checks every row of the shown section at one width: the review dot
    /// (to review only) and the CI dot or dash sit under the first letter
    /// of their heading, in the expected colour.
    fn assert_dots_aligned(app: &mut App, width: u16, ci_shown: bool) {
        let buf = render(app, width);
        let h = headings(&buf);
        let step = if app.config.compact { 1 } else { 2 };
        let expected = samples();
        let shown: Vec<u64> = app.rows(app.section).iter().map(|mr| mr.iid).collect();
        let order: Vec<u64> = expected.iter().map(|(mr, _, _)| mr.iid).collect();
        assert_eq!(shown, order);
        if !ci_shown {
            assert_eq!(h.ci, None, "CI dropped at width {width}");
        }
        for (n, (_, review, ci)) in expected.into_iter().enumerate() {
            let y = h.y + 1 + n as u16 * step;
            if app.section == Section::Review {
                let x = h.review.expect("Review heading");
                assert_eq!(
                    cell_at(&buf, x, y),
                    ("●".to_string(), review),
                    "review dot of row {n} at width {width}"
                );
            }
            if ci_shown {
                let x = h.ci.expect("CI heading");
                assert_eq!(
                    cell_at(&buf, x, y),
                    (ci.0.to_string(), ci.1),
                    "CI of row {n} at width {width}"
                );
            }
        }
    }

    #[test]
    fn dots_start_under_their_heading_to_review() {
        for compact in [false, true] {
            let mut app = app_with(sample_rows(), vec![], compact);
            app.section = Section::Review;
            // Wide (3 cells between columns) and narrow (1 cell).
            assert_dots_aligned(&mut app, 160, true);
            assert_dots_aligned(&mut app, 79, true);
        }
    }

    #[test]
    fn dots_start_under_their_heading_mine() {
        for compact in [false, true] {
            let mut app = app_with(vec![], sample_rows(), compact);
            app.section = Section::Mine;
            assert_dots_aligned(&mut app, 160, true);
            assert_dots_aligned(&mut app, 79, true);
        }
    }

    #[test]
    fn review_dot_stays_under_its_heading_when_ci_is_dropped() {
        for compact in [false, true] {
            let mut app = app_with(sample_rows(), vec![], compact);
            app.section = Section::Review;
            assert_dots_aligned(&mut app, 50, false);
        }
    }

    #[test]
    fn selected_row_keeps_its_dot_in_place() {
        for compact in [false, true] {
            let mut app = app_with(sample_rows(), vec![], compact);
            app.section = Section::Review;
            app.review_state.select(Some(0));
            assert_dots_aligned(&mut app, 160, true);
            let buf = render(&mut app, 160);
            let h = headings(&buf);
            assert_eq!(buf[(h.review.unwrap(), h.y + 1)].bg, SELECTED_BG);
        }
    }

    /// The text of terminal row `y`.
    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The row with the section tabs, and the cells where "Mine" and "To
    /// review" start.
    fn tabs(buf: &Buffer) -> (u16, u16, u16) {
        (0..buf.area.height)
            .find_map(|y| {
                let mine = word_x(buf, y, "Mine")?;
                let review = word_x(buf, y, "To")?;
                word_x(buf, y, "review")?;
                Some((y, mine, review))
            })
            .expect("section tabs")
    }

    #[test]
    fn tabs_show_mine_first_with_counts() {
        let mut app = app_with(sample_rows()[..2].to_vec(), sample_rows(), false);
        let buf = render(&mut app, 120);
        let (y, mine, review) = tabs(&buf);
        assert!(mine < review, "Mine at {mine}, To review at {review}");
        let text = row_text(&buf, y);
        assert!(text.contains(" Mine 6 "), "{text}");
        assert!(text.contains(" To review 2 "), "{text}");
    }

    #[test]
    fn list_opens_on_mine() {
        let mut app = app_with(vec![], sample_rows(), false);
        assert_eq!(app.section, Section::Mine);
        let buf = render(&mut app, 120);
        let (y, mine, review) = tabs(&buf);
        assert_eq!(buf[(mine, y)].bg, SELECTED_BG, "Mine is the active tab");
        assert_ne!(buf[(review, y)].bg, SELECTED_BG);
        // My own rows are listed under the heading bar.
        let h = headings(&buf);
        assert!(row_text(&buf, h.y + 1).contains("Add export"));
        assert!(word_x(&buf, h.y, "Reviewers").is_some());
    }

    #[test]
    fn list_opens_on_mine_when_mine_is_empty() {
        let mut app = app_with(sample_rows(), vec![], false);
        let buf = render(&mut app, 120);
        let (y, mine, _) = tabs(&buf);
        assert_eq!(buf[(mine, y)].bg, SELECTED_BG, "Mine is the active tab");
        let shown = (0..buf.area.height)
            .any(|y| row_text(&buf, y).contains("No open merge requests of yours"));
        assert!(shown, "empty message for my own list");
    }

    #[test]
    fn help_names_the_three_sections() {
        let mut app = app_with(vec![], vec![], false);
        app.help = true;
        let buf = render(&mut app, 120);
        let found = (0..buf.area.height).any(|y| {
            let text = row_text(&buf, y);
            text.contains("Tab  1 2 3") && text.contains("mine / to review / all")
        });
        assert!(found, "help line for Tab 1 2 3");
    }

    /// The list with "All" loaded for `of`, shown on the "All" tab.
    fn app_with_all(items: Vec<MergeRequest>, total: u64, of: Option<Project>) -> App {
        let mut app = app_with(sample_rows()[..2].to_vec(), sample_rows(), false);
        app.project = of.clone();
        app.data.as_mut().unwrap().all = Some(AllList {
            of,
            newest: items.len(),
            items,
            total,
            error: None,
        });
        app.section = Section::All;
        app
    }

    fn acme_app() -> Project {
        Project {
            host: "gitlab.com".into(),
            path: "acme/app".into(),
        }
    }

    fn shown(app: &mut App, text: &str) -> bool {
        screen(app, 120, 20).iter().any(|r| r.contains(text))
    }

    #[test]
    fn tabs_show_all_third_with_its_count() {
        let mut app = app_with_all(sample_rows()[..4].to_vec(), 4, None);
        let buf = render(&mut app, 120);
        let (y, mine, review) = tabs(&buf);
        let all = word_x(&buf, y, "All").expect("All tab");
        assert!(mine < review && review < all, "{mine} {review} {all}");
        let text = row_text(&buf, y);
        assert!(text.contains(" All 4 "), "{text}");
        assert_eq!(buf[(all, y)].bg, SELECTED_BG, "All is the active tab");
        let h = headings(&buf);
        assert!(word_x(&buf, h.y, "Author").is_some());
        assert!(row_text(&buf, h.y + 1).contains("@alex"));
    }

    #[test]
    fn all_shows_the_review_dot_only_where_i_review() {
        let mut mine = sample_mr(9, ReviewState::Unreviewed, Pipeline::None);
        mine.author = "me".into();
        mine.reviewers.clear();
        let mut other = sample_mr(8, ReviewState::Unreviewed, Pipeline::None);
        other.author = "sam".into();
        other.reviewers.clear();
        let approved = sample_mr(7, ReviewState::Approved, Pipeline::None);
        let asked = sample_mr(6, ReviewState::Unreviewed, Pipeline::None);
        // A team request: only in "To review", not among the reviewers.
        let mut team = sample_mr(5, ReviewState::Unreviewed, Pipeline::None);
        team.reviewers.clear();
        let mut app = app_with_all(vec![mine, other, approved, asked, team.clone()], 5, None);
        app.data.as_mut().unwrap().review = vec![team];
        for compact in [false, true] {
            app.config.compact = compact;
            let buf = render(&mut app, 160);
            let h = headings(&buf);
            let x = h.review.expect("Review heading");
            let step = if compact { 1 } else { 2 };
            let at = |n: u16| cell_at(&buf, x, h.y + 1 + n * step);
            assert_eq!(at(0).0, " ", "my own: no dot");
            assert_eq!(at(1).0, " ", "someone else's: no dot");
            assert_eq!(at(2), ("●".to_string(), GREEN));
            assert_eq!(at(3), ("●".to_string(), FAINT));
            assert_eq!(at(4), ("●".to_string(), FAINT), "team request");
        }
    }

    #[test]
    fn empty_all_names_the_project() {
        let mut app = app_with_all(vec![], 0, Some(acme_app()));
        assert!(shown(&mut app, "No open merge requests in app"));
        let mut app = app_with_all(vec![], 0, None);
        assert!(shown(&mut app, "No open merge requests in your projects"));
    }

    #[test]
    fn all_shows_why_it_failed_while_mine_still_lists() {
        let mut app = app_with_all(vec![], 0, None);
        app.data.as_mut().unwrap().all.as_mut().unwrap().error = Some("GitLab: boom".into());
        app.error = Some("All: GitLab: boom".into());
        assert!(shown(&mut app, "GitLab: boom"));
        app.section = Section::Mine;
        assert!(shown(&mut app, "Add export"));
    }

    #[test]
    fn all_loads_like_the_other_sections() {
        // Before the first answer.
        let mut app = app_with(vec![], vec![], false);
        app.data = None;
        app.section = Section::All;
        assert!(shown(&mut app, "Loading merge requests…"));
        // Loaded for "your projects" while "this project" is shown.
        let mut app = app_with_all(sample_rows(), 6, None);
        app.project = Some(acme_app());
        let rows = screen(&mut app, 120, 20);
        assert!(rows.iter().any(|r| r.contains("Loading merge requests…")));
        assert!(rows.iter().any(|r| r.contains(" All … ")), "{rows:#?}");
        app.section = Section::Mine;
        assert!(shown(&mut app, "Add export"), "Mine switches at once");
        // The refresh for this project failed: "All" says why.
        app.section = Section::All;
        app.error = Some("glab: no network".into());
        let rows = screen(&mut app, 120, 20);
        // In the table, not only in the footer (the last row).
        assert!(
            rows[..19].iter().any(|r| r.contains("glab: no network")),
            "{rows:#?}"
        );
        app.loading = true;
        assert!(shown(&mut app, "Loading merge requests…"), "trying again");
    }

    #[test]
    fn all_says_when_it_is_cut() {
        let mut app = app_with_all(sample_rows(), 54, None);
        assert!(shown(&mut app, "newest 6 of 54"));
        app.section = Section::Mine;
        assert!(!shown(&mut app, "newest 6 of 54"), "only on All");
        let mut app = app_with_all(sample_rows(), 6, None);
        assert!(!shown(&mut app, "newest"), "nothing cut");
        // Too narrow next to a message: the message wins.
        let mut app = app_with_all(sample_rows(), 54, None);
        app.error = Some("All: GitLab: the merge request list could not be loaded".into());
        let rows = screen(&mut app, 70, 20);
        let footer = &rows[19];
        assert!(footer.contains("could not be loaded"), "{footer}");
        assert!(!footer.contains("newest"), "{footer}");
    }

    /// Every row of a frame of this size, as text.
    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height).map(|y| row_text(&buf, y)).collect()
    }

    /// The list switched to `status`, with merged or closed ones in "Mine",
    /// "To review" and "All" whose merge state still says ready or conflict,
    /// as the forges report it.
    fn app_in(status: Status) -> App {
        let past = |iid, merge_state| {
            let mut mr = sample_mr(iid, ReviewState::Approved, Pipeline::Success);
            mr.status = status;
            mr.merge_state = merge_state;
            mr
        };
        let mut app = app_with(
            vec![past(2, MergeState::Conflict)],
            vec![past(1, MergeState::Ready)],
            false,
        );
        app.status = status;
        app.data.as_mut().unwrap().all = Some(AllList {
            of: None,
            items: vec![past(3, MergeState::Ready)],
            total: 1,
            newest: 1,
            error: None,
        });
        app
    }

    #[test]
    fn header_says_which_state_is_shown() {
        let mut app = app_with(vec![], sample_rows(), false);
        assert!(screen(&mut app, 120, 20)[0].contains("Merge requests   "));
        let mut app = app_in(Status::Merged);
        assert!(screen(&mut app, 120, 20)[0].contains("Merged merge requests"));
        let mut app = app_in(Status::Closed);
        assert!(screen(&mut app, 120, 20)[0].contains("Closed merge requests"));
        // GitHub only: pull requests.
        for mr in app.data.as_mut().unwrap().mine.iter_mut() {
            mr.forge = Forge::GitHub;
        }
        let data = app.data.as_mut().unwrap();
        data.review.clear();
        data.all = None;
        assert!(screen(&mut app, 120, 20)[0].contains("Closed pull requests"));
    }

    #[test]
    fn narrow_header_keeps_the_state_and_the_tabs() {
        let mut app = app_in(Status::Merged);
        // Wide enough for every tab of the open list, not with the long title.
        let header = &screen(&mut app, 54, 20)[0];
        assert!(header.contains("Merged "), "{header}");
        assert!(!header.contains("merge requests"), "{header}");
        assert!(header.contains(" All 1 "), "{header}");
        // Narrower still: the state shows first.
        let header = &screen(&mut app, 30, 20)[0];
        assert!(header.contains("Merged"), "{header}");
        // Open ones keep their title as today.
        let mut app = app_with(vec![], sample_rows(), false);
        let header = &screen(&mut app, 54, 20)[0];
        assert!(header.contains("Merge requests"), "{header}");
    }

    #[test]
    fn merged_and_closed_rows_say_so_never_an_open_status() {
        for (status, word, color) in [
            (Status::Merged, "Merged", ACCENT),
            (Status::Closed, "Closed", RED),
        ] {
            for section in [Section::Mine, Section::Review, Section::All] {
                let mut app = app_in(status);
                app.section = section;
                let rows = screen(&mut app, 120, 20);
                assert!(
                    rows.iter().any(|r| r.contains(&format!("·  {word}"))),
                    "{rows:#?}"
                );
                for open_only in ["Ready to merge", "Merge conflict"] {
                    assert!(!rows.iter().any(|r| r.contains(open_only)), "{open_only}");
                }
            }
            // The state icon too, also in compact rows where it is all there is.
            let mut app = app_in(status);
            app.config.compact = true;
            app.config.nerd_font = false;
            let buf = render(&mut app, 120);
            let (icon, c) = Icons { nerd: false }.status(status).unwrap();
            let y = (0..buf.area.height)
                .find(|&y| row_text(&buf, y).contains("Add export"))
                .unwrap();
            let x = (0..buf.area.width).find(|&x| buf[(x, y)].symbol() == icon);
            assert!(
                x.is_some_and(|x| buf[(x, y)].fg == c),
                "{}",
                row_text(&buf, y)
            );
            assert_eq!(
                status_text(&app.rows(Section::Mine)[0].clone()),
                (word, color)
            );
        }
    }

    #[test]
    fn details_of_a_merged_one_offer_no_writes() {
        let with_clone = |app: &mut App| {
            app.details = true;
            app.mine_state.select(Some(0));
            app.herdr = true;
            app.workdir = Some(std::path::PathBuf::from("/src/app"));
            app.remotes = vec![crate::repo::Remote {
                name: "origin".into(),
                project: Project {
                    host: "gitlab.com".into(),
                    path: "acme/app".into(),
                },
            }];
        };
        let actions = |app: &mut App| {
            let rows = screen(app, 160, 40);
            let line = rows
                .iter()
                .find(|r| r.contains("d changes"))
                .cloned()
                .expect("actions");
            (line, rows)
        };
        let mut app = app_in(Status::Merged);
        with_clone(&mut app);
        let (line, rows) = actions(&mut app);
        for write in ["v approve", "v revoke", "m merge", "b branch"] {
            assert!(!line.contains(write), "{line}");
        }
        assert!(
            !rows
                .iter()
                .any(|r| r.contains("not approved yet") || r.contains("waiting for")),
            "{rows:#?}"
        );
        assert!(rows.iter().any(|r| r.contains("Merged")));
        // Open ones keep their hints.
        let mut ready = sample_mr(1, ReviewState::Unreviewed, Pipeline::Success);
        ready.merge_state = MergeState::Ready;
        let mut app = app_with(vec![], vec![ready], false);
        with_clone(&mut app);
        let (line, _) = actions(&mut app);
        for write in ["v approve", "m merge", "b branch"] {
            assert!(line.contains(write), "{line}");
        }
    }

    #[test]
    fn merged_and_closed_lists_say_what_is_missing_or_loading() {
        let mut app = app_in(Status::Merged);
        app.data = None;
        app.loading = true;
        assert!(shown(&mut app, "Loading merged merge requests…"));
        assert!(!shown(&mut app, "Nothing to review"));
        let empty = |status| {
            let mut app = app_in(status);
            let data = app.data.as_mut().unwrap();
            data.review.clear();
            data.mine.clear();
            data.all.as_mut().unwrap().items.clear();
            app
        };
        let mut app = empty(Status::Merged);
        assert!(shown(&mut app, "No merged merge requests of yours"));
        app.section = Section::Review;
        assert!(shown(
            &mut app,
            "No merged merge requests with you as reviewer"
        ));
        app.section = Section::All;
        assert!(shown(&mut app, "No merged merge requests in your projects"));
        // This project, with the hint for all projects.
        let mut app = empty(Status::Closed);
        let project = Project {
            host: "gitlab.com".into(),
            path: "acme/app".into(),
        };
        app.project = Some(project.clone());
        app.data.as_mut().unwrap().of = Some(project.clone());
        app.data.as_mut().unwrap().all.as_mut().unwrap().of = Some(project);
        assert!(shown(
            &mut app,
            "No closed merge requests of yours in app  (a shows all projects)"
        ));
        // Loaded for the other scope: loading, not empty.
        app.data.as_mut().unwrap().of = None;
        app.loading = true;
        assert!(shown(&mut app, "Loading closed merge requests…"));
        assert!(screen(&mut app, 120, 20)[0].contains(" Mine … "));
    }

    #[test]
    fn a_long_empty_message_wraps_instead_of_being_cut() {
        let mut app = app_in(Status::Closed);
        let data = app.data.as_mut().unwrap();
        data.mine.clear();
        let project = Project {
            host: "gitlab.com".into(),
            path: "acme/herdr-git-panel".into(),
        };
        app.project = Some(project.clone());
        app.data.as_mut().unwrap().of = Some(project);
        for width in [60, 25] {
            let rows = screen(&mut app, width, 20);
            let words: Vec<&str> = rows.iter().flat_map(|r| r.split_whitespace()).collect();
            let text = words.join(" ");
            let message =
                "No closed merge requests of yours in herdr-git-panel (a shows all projects)";
            assert!(text.contains(message), "{rows:#?}");
        }
    }

    #[test]
    fn help_names_the_state_key() {
        let mut app = app_with(vec![], vec![], false);
        app.help = true;
        let rows = screen(&mut app, 120, 50);
        assert!(rows
            .iter()
            .any(|r| r.contains("z ") && r.contains("open / merged / closed")));
        assert!(rows.iter().any(|r| r.contains("b v m w: open ones only")));
    }

    #[test]
    fn help_names_b() {
        let mut app = app_with(vec![], vec![], false);
        app.help = true;
        let rows = screen(&mut app, 120, 50);
        assert!(
            rows.iter().any(|r| r.contains("b ")
                && r.contains("open the branch in a new Herdr workspace")),
            "{rows:#?}"
        );
    }

    #[test]
    fn details_offer_b_only_for_the_lists_own_clone() {
        let mut app = app_with(vec![], sample_rows(), false);
        app.details = true;
        app.mine_state.select(Some(0));
        let offered = |app: &mut App| {
            let rows = screen(app, 160, 40);
            assert!(rows.iter().any(|r| r.contains("d changes")), "{rows:#?}");
            rows.iter().any(|r| r.contains(" b branch"))
        };
        assert!(!offered(&mut app), "no clone of acme/app here");
        app.workdir = Some(std::path::PathBuf::from("/src/app"));
        app.remotes = vec![crate::repo::Remote {
            name: "upstream".into(),
            project: crate::repo::Project {
                host: "gitlab.com".into(),
                path: "Acme/App".into(),
            },
        }];
        assert!(!offered(&mut app), "not inside Herdr");
        app.herdr = true;
        assert!(offered(&mut app));
    }
}
