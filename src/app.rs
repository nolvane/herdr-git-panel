//! The list's state and its event loop: keys, mouse, approvals, automatic
//! and manual refresh.

use crate::config::{Config, Scope};
use crate::forge::{self, Fetched};
use crate::model::{Data, Forge, MergeRequest, MergeState};
use crate::repo::Project;
use crate::ui;
use crate::view::{self, Show, SortKey};
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::TableState;
use ratatui::DefaultTerminal;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Review,
    Mine,
}

/// Something on screen a mouse click or the wheel acts on, recorded while
/// drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Section(Section),
    Scope,
    /// A column heading: sorts by it, a second click turns the order round.
    Sort(SortKey),
    /// A table row, by its index in the current section.
    Row(usize),
    /// The details panel (the wheel scrolls it).
    Details,
    /// A key hint in the footer or a button: acts like the key.
    Key(char),
    /// A tab of the details panel.
    Panel(PanelTab),
    /// A thread in the activity tab (index into its discussions): select.
    Thread(usize),
    /// The resolve button of a thread.
    Resolve(usize),
    /// A filter chip in the activity tab.
    Threads(ThreadFilter),
    /// A file in the changes viewer's tree.
    File(usize),
    /// A link in the details panel (index into `App::links`).
    Link(usize),
    /// Anywhere while the help is open: closes it.
    Help,
}

/// What waits for confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Approve,
    Revoke,
    Merge,
}

/// An action on a merge request waiting for y or n.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub action: Action,
    /// The merge request as the user saw it; a merge only goes through on
    /// its head commit.
    pub mr: MergeRequest,
}

/// The two views of the details panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Overview,
    Activity,
}

/// Comments of one merge request, as far as loaded.
#[derive(Debug, Clone)]
pub enum Activity {
    Loading,
    Loaded(Vec<crate::model::Discussion>),
    Failed(String),
}

type ActivityKey = (String, u64);

/// Which threads the activity tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadFilter {
    /// Everything: threads, comments and GitLab's own notes.
    All,
    Open,
    Resolved,
}

impl ThreadFilter {
    pub fn keeps(self, d: &crate::model::Discussion) -> bool {
        match self {
            ThreadFilter::All => true,
            ThreadFilter::Open => d.resolvable && !d.resolved,
            ThreadFilter::Resolved => d.resolvable && d.resolved,
        }
    }

    fn next(self) -> ThreadFilter {
        match self {
            ThreadFilter::All => ThreadFilter::Open,
            ThreadFilter::Open => ThreadFilter::Resolved,
            ThreadFilter::Resolved => ThreadFilter::All,
        }
    }
}

/// The built-in changes viewer.
pub struct DiffView {
    pub iid: u64,
    /// `!42` or `#42`.
    pub reference: String,
    pub title: String,
    pub diff: Option<Result<crate::diff::Diff, String>>,
    /// First line shown.
    pub scroll: usize,
    /// Lines that fit on screen, set while drawing.
    pub height: usize,
    /// The file tree on the left is shown.
    pub tree: bool,
}

impl DiffView {
    fn len(&self) -> usize {
        match &self.diff {
            Some(Ok(d)) => d.lines.len(),
            _ => 0,
        }
    }

    fn scroll_by(&mut self, delta: i64) {
        let max = self.len().saturating_sub(self.height.max(1)) as i64;
        self.scroll = (self.scroll as i64 + delta).clamp(0, max.max(0)) as usize;
    }

    /// The file the top line belongs to (index into `files`).
    pub fn current_file(&self) -> Option<usize> {
        let Some(Ok(d)) = &self.diff else {
            return None;
        };
        d.files.iter().rposition(|f| f.at <= self.scroll)
    }

    /// Shows the file with this index at the top.
    pub fn goto_file(&mut self, index: usize) {
        if let Some(Ok(d)) = &self.diff {
            if let Some(f) = d.files.get(index) {
                self.scroll = f.at;
                self.scroll_by(0);
            }
        }
    }

    fn jump_file(&mut self, forward: bool) {
        let Some(Ok(d)) = &self.diff else {
            return;
        };
        let target = if forward {
            d.files.iter().find(|f| f.at > self.scroll)
        } else {
            d.files.iter().rev().find(|f| f.at < self.scroll)
        };
        if let Some(f) = target {
            self.scroll = f.at;
            self.scroll_by(0);
        }
    }
}

pub struct App {
    pub config: Config,
    pub project: Option<Project>,
    pub scope: Scope,
    pub section: Section,
    pub review_state: TableState,
    pub mine_state: TableState,
    pub data: Option<Data>,
    pub error: Option<String>,
    pub loading: bool,
    /// When the data was last loaded (wall clock, for "updated 12s ago").
    pub updated_at: Option<Instant>,
    pub next_refresh: Instant,
    /// A short message and until when it shows.
    pub flash: Option<(String, Instant)>,
    config_dir: Option<std::path::PathBuf>,
    /// The details panel is shown.
    pub details: bool,
    /// How far the details panel is scrolled.
    pub details_scroll: u16,
    /// An action waiting for y or n.
    pub confirm: Option<Confirm>,
    /// q was pressed: closing waits for y, so the panel is not closed by
    /// accident.
    pub ask_quit: bool,
    /// The overlay with all keys is open.
    pub help: bool,
    pub panel_tab: PanelTab,
    pub thread_filter: ThreadFilter,
    /// The selected thread in the activity tab (index into the discussions).
    pub thread: Option<usize>,
    /// Comments per merge request, with the update time they were loaded for.
    pub activity: std::collections::HashMap<ActivityKey, (Option<i64>, Activity)>,
    activity_tx: Sender<(ActivityKey, Result<Vec<crate::model::Discussion>, String>)>,
    resolve_tx: Sender<(ActivityKey, usize, bool, Result<(), String>)>,
    resolve_rx: Receiver<(ActivityKey, usize, bool, Result<(), String>)>,
    /// Scroll the activity tab to the selected thread on the next frame.
    pub scroll_to_thread: bool,
    activity_rx: Receiver<(ActivityKey, Result<Vec<crate::model::Discussion>, String>)>,
    pub sort: SortKey,
    pub descending: bool,
    pub show: Show,
    /// Text typed after `/`; every word must match.
    pub filter: String,
    /// Keys go into the filter while typing it.
    pub typing: bool,
    /// Link targets of the last frame, for `Hit::Link`.
    pub links: Vec<String>,
    /// Clickable areas of the last frame.
    pub hits: Vec<(Rect, Hit)>,
    last_click: Option<(usize, Instant)>,
    /// The changes viewer, when open.
    pub diff: Option<DiffView>,
    diff_tx: Sender<(u64, Result<crate::diff::Diff, String>)>,
    diff_rx: Receiver<(u64, Result<crate::diff::Diff, String>)>,
    act_tx: Sender<Result<String, String>>,
    act_rx: Receiver<Result<String, String>>,
    tx: Sender<Result<Fetched, String>>,
    rx: Receiver<Result<Fetched, String>>,
    quit: bool,
}

impl App {
    pub fn new(
        config: Config,
        project: Option<Project>,
        config_dir: Option<std::path::PathBuf>,
    ) -> App {
        let (tx, rx) = mpsc::channel();
        let (act_tx, act_rx) = mpsc::channel();
        let (activity_tx, activity_rx) = mpsc::channel();
        let (diff_tx, diff_rx) = mpsc::channel();
        let (resolve_tx, resolve_rx) = mpsc::channel();
        let scope = config.scope;
        // Until icons are chosen, say how to fix "?" or boxes in place of icons.
        let flash = (!config.icons_chosen).then(|| {
            (
                "No icons, only ? or boxes? Press i for plain symbols".to_string(),
                Instant::now() + Duration::from_secs(15),
            )
        });
        App {
            config,
            project,
            scope,
            section: Section::Review,
            review_state: TableState::default(),
            mine_state: TableState::default(),
            data: None,
            error: None,
            loading: false,
            updated_at: None,
            next_refresh: Instant::now(),
            flash,
            config_dir,
            details: false,
            details_scroll: 0,
            confirm: None,
            ask_quit: false,
            help: false,
            panel_tab: PanelTab::Overview,
            thread_filter: ThreadFilter::All,
            thread: None,
            activity: std::collections::HashMap::new(),
            activity_tx,
            activity_rx,
            resolve_tx,
            resolve_rx,
            scroll_to_thread: false,
            hits: Vec::new(),
            links: Vec::new(),
            sort: SortKey::Updated,
            descending: true,
            show: Show::All,
            filter: String::new(),
            typing: false,
            last_click: None,
            diff: None,
            diff_tx,
            diff_rx,
            act_tx,
            act_rx,
            tx,
            rx,
            quit: false,
        }
    }

    /// "Pull requests" when everything in view is on GitHub, else "Merge
    /// requests" (GitLab's word, also for a mix of both).
    pub fn plural_noun(&self) -> &'static str {
        let github = match self.scoped_project() {
            Some(p) => p.host == crate::github::HOST,
            None => self.data.as_ref().is_some_and(|d| {
                let mut all = d.review.iter().chain(d.mine.iter()).peekable();
                all.peek().is_some() && all.all(|mr| mr.forge == Forge::GitHub)
            }),
        };
        if github {
            "Pull requests"
        } else {
            "Merge requests"
        }
    }

    /// Whether the list mixes GitLab and GitHub, so rows say where they live.
    pub fn mixed_forges(&self) -> bool {
        self.scoped_project().is_none()
            && self.data.as_ref().is_some_and(|d| {
                let all: Vec<Forge> = d
                    .review
                    .iter()
                    .chain(d.mine.iter())
                    .map(|m| m.forge)
                    .collect();
                all.contains(&Forge::GitLab) && all.contains(&Forge::GitHub)
            })
    }

    /// Whether the list is limited to the current project right now.
    pub fn scoped_project(&self) -> Option<&Project> {
        match self.scope {
            Scope::Current => self.project.as_ref(),
            Scope::All => None,
        }
    }

    /// The merge requests of a section as shown: scope, quick filter and
    /// typed filter applied, then sorted.
    pub fn rows(&self, section: Section) -> Vec<&MergeRequest> {
        let Some(data) = &self.data else {
            return Vec::new();
        };
        let list = match section {
            Section::Review => &data.review,
            Section::Mine => &data.mine,
        };
        let reviewing = section == Section::Review;
        let mut rows: Vec<&MergeRequest> = list
            .iter()
            .filter(|mr| match self.scoped_project() {
                Some(p) => mr.project == p.path && mr.host == p.host,
                None => true,
            })
            .filter(|mr| self.show.keeps(mr, &mr.viewer, reviewing))
            .filter(|mr| view::matches(mr, &self.filter))
            .collect();
        view::sort(&mut rows, self.sort, self.descending);
        rows
    }

    /// Whether any filter hides merge requests right now.
    pub fn filtered(&self) -> bool {
        self.show != Show::All || !self.filter.trim().is_empty()
    }

    fn sort_by(&mut self, key: SortKey) {
        if self.sort == key {
            self.descending = !self.descending;
        } else {
            self.sort = key;
            self.descending = key.descending_by_default();
        }
        self.after_view_change();
    }

    /// After sorting or filtering: back to the top of each list.
    fn after_view_change(&mut self) {
        self.review_state.select(Some(0));
        self.mine_state.select(Some(0));
        *self.review_state.offset_mut() = 0;
        *self.mine_state.offset_mut() = 0;
        self.details_scroll = 0;
        self.clamp_selection();
    }

    pub fn state_mut(&mut self) -> &mut TableState {
        match self.section {
            Section::Review => &mut self.review_state,
            Section::Mine => &mut self.mine_state,
        }
    }

    pub fn selected(&self) -> Option<&MergeRequest> {
        let state = match self.section {
            Section::Review => &self.review_state,
            Section::Mine => &self.mine_state,
        };
        let rows = self.rows(self.section);
        state.selected().and_then(|i| rows.get(i).copied())
    }

    fn refresh(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        let tx = self.tx.clone();
        let host = self.config.gitlab_host.clone();
        std::thread::spawn(move || {
            let _ = tx.send(forge::fetch_all(&host));
        });
    }

    fn receive_actions(&mut self) {
        while let Ok(result) = self.act_rx.try_recv() {
            match result {
                Ok(msg) => {
                    self.say(msg);
                    self.loading = false;
                    self.refresh();
                }
                Err(e) => self.say(e),
            }
        }
    }

    /// Asks for confirmation before approving (or revoking) the selected
    /// merge request.
    fn ask_approval(&mut self) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        if mr.author == mr.viewer {
            self.say(format!("You cannot approve your own {}", mr.forge.noun()));
            return;
        }
        let action = if mr.approved_by.contains(&mr.viewer) {
            if !forge::can_revoke(mr.forge) {
                self.say(format!(
                    "You approved {} already; GitHub cannot take an approval back",
                    mr.reference()
                ));
                return;
            }
            Action::Revoke
        } else {
            Action::Approve
        };
        self.confirm = Some(Confirm { action, mr });
    }

    /// Asks for confirmation before merging; only when GitLab says the merge
    /// request can be merged.
    fn ask_merge(&mut self) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        if mr.merge_state != MergeState::Ready {
            let forge = mr.forge.name();
            let why = match mr.merge_state {
                MergeState::Draft => "it is a draft".to_string(),
                MergeState::Conflict => "it has a merge conflict".to_string(),
                MergeState::NeedsRebase => "it needs a rebase".to_string(),
                MergeState::Checking => format!("{forge} is still checking it"),
                _ if mr.threads_open > 0 => "threads are still open".to_string(),
                _ if !mr.approved => "it is not approved yet".to_string(),
                _ => format!("{forge} does not allow it yet"),
            };
            self.say(format!("Cannot merge {}: {why}", mr.reference()));
            return;
        }
        self.confirm = Some(Confirm {
            action: Action::Merge,
            mr,
        });
    }

    /// Marks your own merge request as draft, or as ready again. No
    /// confirmation: it is undone with the same key.
    fn toggle_draft(&mut self) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        if mr.author != mr.viewer {
            self.say(format!(
                "Only your own {}s can be set to draft or ready here",
                mr.forge.noun()
            ));
            return;
        }
        let draft = mr.merge_state != MergeState::Draft;
        self.say(if draft {
            "Marking as draft…"
        } else {
            "Marking as ready…"
        });
        let tx = self.act_tx.clone();
        std::thread::spawn(move || {
            let r = mr.reference();
            let result = forge::set_draft(&mr, draft).map(|()| {
                if draft {
                    format!("{r} is a draft now")
                } else {
                    format!("{r} is ready for review")
                }
            });
            let _ = tx.send(result);
        });
    }

    fn confirm_action(&mut self) {
        let Some(c) = self.confirm.take() else {
            return;
        };
        let tx = self.act_tx.clone();
        self.say(match c.action {
            Action::Approve => "Approving…",
            Action::Revoke => "Revoking…",
            Action::Merge => "Merging…",
        });
        std::thread::spawn(move || {
            let r = c.mr.reference();
            let result = match c.action {
                Action::Approve => forge::approve(&c.mr).map(|()| format!("{r} approved")),
                Action::Revoke => forge::revoke(&c.mr).map(|()| format!("Approval of {r} revoked")),
                Action::Merge => forge::merge(&c.mr).map(|()| format!("{r} merged")),
            };
            let _ = tx.send(result);
        });
    }

    /// Loads the comments of the selected merge request when the activity
    /// tab shows it and they are missing or older than its last update.
    fn load_activity(&mut self) {
        if !self.details || self.panel_tab != PanelTab::Activity {
            return;
        }
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        let key = mr.key();
        let updated = mr.updated;
        match self.activity.get(&key) {
            Some((_, Activity::Loading)) => return,
            Some((at, _)) if *at == updated => return,
            _ => {}
        }
        self.activity
            .insert(key.clone(), (updated, Activity::Loading));
        let tx = self.activity_tx.clone();
        std::thread::spawn(move || {
            let result = forge::fetch_activity(&mr);
            let _ = tx.send((key, result));
        });
    }

    /// Comments that failed to load are tried again the next time they show.
    fn forget_failed_activity(&mut self) {
        self.activity
            .retain(|_, (_, a)| !matches!(a, Activity::Failed(_)));
    }

    fn receive_activity(&mut self) {
        while let Ok((key, result)) = self.activity_rx.try_recv() {
            let updated = self.activity.get(&key).and_then(|(u, _)| *u);
            let value = match result {
                Ok(list) => Activity::Loaded(list),
                Err(e) => Activity::Failed(e),
            };
            self.activity.insert(key, (updated, value));
        }
    }

    /// The loaded comments of the selected merge request.
    pub fn selected_activity(&self) -> Option<&Activity> {
        let mr = self.selected()?;
        self.activity.get(&mr.key()).map(|(_, a)| a)
    }

    /// Indexes of the threads (resolvable discussions) the filter shows.
    pub fn visible_threads(&self) -> Vec<usize> {
        match self.selected_activity() {
            Some(Activity::Loaded(list)) => list
                .iter()
                .enumerate()
                .filter(|(_, d)| d.resolvable && self.thread_filter.keeps(d))
                .map(|(i, _)| i)
                .collect(),
            _ => Vec::new(),
        }
    }

    fn step_thread(&mut self, forward: bool) {
        let threads = self.visible_threads();
        if threads.is_empty() {
            return;
        }
        let pos = self
            .thread
            .and_then(|t| threads.iter().position(|i| *i == t));
        let next = match (pos, forward) {
            (None, _) => 0,
            (Some(p), true) => (p + 1).min(threads.len() - 1),
            (Some(p), false) => p.saturating_sub(1),
        };
        self.thread = Some(threads[next]);
        self.scroll_to_thread = true;
    }

    /// Resolves the thread, or opens it again when it is resolved.
    fn toggle_resolved(&mut self, index: usize) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        let key = mr.key();
        let Some((_, Activity::Loaded(list))) = self.activity.get_mut(&key) else {
            return;
        };
        let Some(d) = list.get_mut(index) else {
            return;
        };
        if !d.resolvable {
            return;
        }
        let resolve = !d.resolved;
        // Show it at once; a failure puts it back.
        d.resolved = resolve;
        let id = d.id.clone();
        self.thread = Some(index);
        let tx = self.resolve_tx.clone();
        std::thread::spawn(move || {
            let result = forge::set_resolved(&mr, &id, resolve);
            let _ = tx.send((key, index, resolve, result));
        });
    }

    fn receive_resolved(&mut self) {
        while let Ok((key, index, resolve, result)) = self.resolve_rx.try_recv() {
            match result {
                Ok(()) => {
                    self.say(if resolve {
                        "Thread resolved"
                    } else {
                        "Thread opened again"
                    });
                    // The list's thread counts change too.
                    self.loading = false;
                    self.refresh();
                }
                Err(e) => {
                    if let Some((_, Activity::Loaded(list))) = self.activity.get_mut(&key) {
                        if let Some(d) = list.get_mut(index) {
                            d.resolved = !resolve;
                        }
                    }
                    self.say(e);
                }
            }
        }
    }

    fn in_activity(&self) -> bool {
        self.details && self.panel_tab == PanelTab::Activity
    }

    fn switch_panel(&mut self, tab: PanelTab) {
        self.details = true;
        self.panel_tab = tab;
        self.details_scroll = 0;
    }

    /// Opens the changes viewer for the selected merge request.
    fn show_changes(&mut self) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        let (iid, title) = (mr.iid, mr.title.clone());
        self.diff = Some(DiffView {
            iid,
            reference: mr.reference(),
            title,
            diff: None,
            scroll: 0,
            height: 20,
            tree: true,
        });
        let tx = self.diff_tx.clone();
        std::thread::spawn(move || {
            let result = forge::fetch_diff(&mr).map(|t| crate::diff::parse(&t));
            let _ = tx.send((iid, result));
        });
    }

    fn receive_diff(&mut self) {
        while let Ok((iid, result)) = self.diff_rx.try_recv() {
            if let Some(view) = self.diff.as_mut().filter(|v| v.iid == iid) {
                view.diff = Some(result);
            }
        }
    }

    /// Keys while the changes viewer is open.
    fn diff_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        let Some(view) = self.diff.as_mut() else {
            return;
        };
        let page = view.height.max(2) as i64 - 1;
        match code {
            KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => view.scroll_by(page / 2),
            KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => view.scroll_by(-page / 2),
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('d') => self.diff = None,
            KeyCode::Down | KeyCode::Char('j') => view.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => view.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => view.scroll_by(page),
            KeyCode::PageUp => view.scroll_by(-page),
            KeyCode::Home | KeyCode::Char('g') => view.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => view.scroll_by(i64::MAX / 2),
            KeyCode::Char('n') | KeyCode::Right | KeyCode::Tab => view.jump_file(true),
            KeyCode::Char('N') | KeyCode::Left | KeyCode::BackTab => view.jump_file(false),
            KeyCode::Char('o') | KeyCode::Enter => self.open_selected(),
            KeyCode::Char('f') => view.tree = !view.tree,
            _ => {}
        }
    }

    fn select(&mut self, index: usize) {
        let len = self.rows(self.section).len();
        if index < len {
            if self.state_mut().selected() != Some(index) {
                self.details_scroll = 0;
                self.thread = None;
                self.forget_failed_activity();
            }
            self.state_mut().select(Some(index));
        }
    }

    fn mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(pos))
            .map(|(_, h)| *h);
        if let Some(view) = self.diff.as_mut() {
            match m.kind {
                MouseEventKind::ScrollDown => view.scroll_by(3),
                MouseEventKind::ScrollUp => view.scroll_by(-3),
                MouseEventKind::Down(MouseButton::Left) => match hit {
                    Some(Hit::Key(c)) => self.diff_key(KeyCode::Char(c), KeyModifiers::NONE),
                    Some(Hit::File(i)) => view.goto_file(i),
                    _ => {}
                },
                _ => {}
            }
            return;
        }
        if self.help {
            if let MouseEventKind::Down(_) = m.kind {
                self.help = false;
            }
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => match hit {
                Some(Hit::Panel(tab)) => self.switch_panel(tab),
                Some(Hit::Thread(i)) => self.thread = Some(i),
                Some(Hit::Link(i)) => {
                    if let Some(url) = self.links.get(i).cloned() {
                        self.open_url(&url);
                    }
                }
                Some(Hit::Resolve(i)) => self.toggle_resolved(i),
                Some(Hit::Threads(f)) => {
                    self.thread_filter = f;
                    self.thread = None;
                    self.details_scroll = 0;
                }
                Some(Hit::Section(s)) => self.section = s,
                Some(Hit::Scope) => self.toggle_scope(),
                Some(Hit::Sort(k)) => self.sort_by(k),
                Some(Hit::Row(i)) => {
                    // A second click on the same row within half a second
                    // opens it in the browser.
                    let double = matches!(self.last_click, Some((j, at)) if j == i && at.elapsed() < Duration::from_millis(500));
                    self.select(i);
                    if double {
                        self.last_click = None;
                        self.open_selected();
                    } else {
                        self.last_click = Some((i, Instant::now()));
                    }
                }
                Some(Hit::Key(c)) => self.key(KeyCode::Char(c), KeyModifiers::NONE),
                _ => {}
            },
            MouseEventKind::ScrollDown => match hit {
                Some(Hit::Details | Hit::Thread(_) | Hit::Resolve(_) | Hit::Threads(_)) => {
                    self.details_scroll = self.details_scroll.saturating_add(2)
                }
                _ => self.move_by(1),
            },
            MouseEventKind::ScrollUp => match hit {
                Some(Hit::Details | Hit::Thread(_) | Hit::Resolve(_) | Hit::Threads(_)) => {
                    self.details_scroll = self.details_scroll.saturating_sub(2)
                }
                _ => self.move_by(-1),
            },
            _ => {}
        }
    }

    fn receive(&mut self) {
        while let Ok(result) = self.rx.try_recv() {
            self.loading = false;
            self.next_refresh = Instant::now() + Duration::from_secs(self.config.refresh_secs);
            match result {
                Ok(fetched) => {
                    self.data = Some(fetched.data);
                    self.error = fetched.warning;
                    self.updated_at = Some(Instant::now());
                    self.clamp_selection();
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// Keeps each section's selection inside its list.
    fn clamp_selection(&mut self) {
        for section in [Section::Review, Section::Mine] {
            let len = self.rows(section).len();
            let state = match section {
                Section::Review => &mut self.review_state,
                Section::Mine => &mut self.mine_state,
            };
            match (len, state.selected()) {
                (0, _) => state.select(None),
                (_, None) => state.select(Some(0)),
                (n, Some(i)) if i >= n => state.select(Some(n - 1)),
                _ => {}
            }
        }
    }

    fn move_by(&mut self, delta: i64) {
        let len = self.rows(self.section).len() as i64;
        if len == 0 {
            return;
        }
        let current = self.state_mut().selected().unwrap_or(0) as i64;
        self.select((current + delta).clamp(0, len - 1) as usize);
    }

    fn jump(&mut self, to_end: bool) {
        let len = self.rows(self.section).len();
        if len == 0 {
            return;
        }
        self.select(if to_end { len - 1 } else { 0 });
    }

    fn switch_section(&mut self) {
        self.section = match self.section {
            Section::Review => Section::Mine,
            Section::Mine => Section::Review,
        };
    }

    fn say(&mut self, text: impl Into<String>) {
        self.flash = Some((text.into(), Instant::now() + Duration::from_secs(3)));
    }

    fn open_selected(&mut self) {
        let Some(url) = self.selected().map(|mr| mr.url.clone()) else {
            return;
        };
        self.open_url(&url);
    }

    /// Opens a link in the browser.
    fn open_url(&mut self, url: &str) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let ok = Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok();
        self.say(if ok {
            "Opened in the browser".to_string()
        } else {
            format!("Could not run {opener}")
        });
    }

    fn copy_selected(&mut self) {
        let Some(url) = self.selected().map(|mr| mr.url.clone()) else {
            return;
        };
        let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
            ("pbcopy", &[])
        } else {
            ("xclip", &["-selection", "clipboard"])
        };
        let ok = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write_all(url.as_bytes())?;
                }
                child.wait()
            })
            .map(|s| s.success())
            .unwrap_or(false);
        self.say(if ok {
            "Link copied"
        } else {
            "Could not copy the link"
        });
    }

    fn toggle_scope(&mut self) {
        if self.project.is_none() {
            self.say("No GitLab or GitHub project in this directory: showing all");
            return;
        }
        self.scope = match self.scope {
            Scope::Current => Scope::All,
            Scope::All => Scope::Current,
        };
        self.review_state.select(Some(0));
        self.mine_state.select(Some(0));
        self.clamp_selection();
    }

    /// Switches between Nerd Font icons and plain symbols, and keeps the
    /// choice in config.toml.
    fn toggle_icons(&mut self) {
        self.config.nerd_font = !self.config.nerd_font;
        self.config.icons_chosen = true;
        let saved = self
            .config_dir
            .as_deref()
            .map(|d| crate::config::save_icons(d, self.config.nerd_font).is_ok())
            .unwrap_or(false);
        let what = if self.config.nerd_font {
            "Nerd Font icons"
        } else {
            "Plain symbols"
        };
        self.say(if saved {
            format!("{what}, saved")
        } else {
            format!("{what}, not saved")
        });
    }

    fn key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if self.diff.is_some() {
            self.diff_key(code, mods);
            return;
        }
        // While typing a filter, keys go into it.
        if self.typing {
            match code {
                KeyCode::Enter => self.typing = false,
                KeyCode::Esc => {
                    self.typing = false;
                    self.filter.clear();
                    self.after_view_change();
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.after_view_change();
                }
                KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => {
                    self.filter.push(c);
                    self.after_view_change();
                }
                _ => {}
            }
            return;
        }
        // The help overlay closes on any key.
        if self.help {
            self.help = false;
            return;
        }
        // Closing the panel waits for y; any other key keeps it open.
        if self.ask_quit {
            self.ask_quit = false;
            if matches!(code, KeyCode::Char('y') | KeyCode::Enter) {
                self.quit = true;
            }
            return;
        }
        // While an approval waits for an answer, only y and n count.
        if self.confirm.is_some() {
            match code {
                KeyCode::Char('y') | KeyCode::Enter => self.confirm_action(),
                KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.confirm = None;
                    self.say("Cancelled");
                }
                _ => {}
            }
            return;
        }
        match code {
            KeyCode::Char('d') if mods.contains(KeyModifiers::CONTROL) => {
                self.details_scroll = self.details_scroll.saturating_add(5)
            }
            KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => {
                self.details_scroll = self.details_scroll.saturating_sub(5)
            }
            KeyCode::Char('p') | KeyCode::Char(' ') => {
                self.details = !self.details;
                self.details_scroll = 0;
            }
            KeyCode::Char('n') if self.in_activity() => self.step_thread(true),
            KeyCode::Char('N') if self.in_activity() => self.step_thread(false),
            KeyCode::Char('t') if self.in_activity() => {
                self.thread_filter = self.thread_filter.next();
                self.thread = None;
                self.details_scroll = 0;
            }
            KeyCode::Char('e') if self.in_activity() => {
                if let Some(t) = self.thread {
                    self.toggle_resolved(t);
                } else {
                    self.say("Choose a thread first: n");
                }
            }
            KeyCode::Char('v') => self.ask_approval(),
            KeyCode::Char('m') => self.ask_merge(),
            KeyCode::Char('w') => self.toggle_draft(),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('c') if !mods.contains(KeyModifiers::CONTROL) => {
                let tab = if self.details && self.panel_tab == PanelTab::Activity {
                    PanelTab::Overview
                } else {
                    PanelTab::Activity
                };
                self.switch_panel(tab);
            }
            KeyCode::Char('d') => self.show_changes(),
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Char('f') => {
                self.show = self.show.next();
                self.after_view_change();
            }
            KeyCode::Char('s') => {
                let next = self.sort.next();
                self.sort = next;
                self.descending = next.descending_by_default();
                self.after_view_change();
            }
            KeyCode::Char('x') => {
                self.filter.clear();
                self.show = Show::All;
                self.after_view_change();
                self.say("Cleared");
            }
            KeyCode::Esc if self.details => self.details = false,
            KeyCode::Esc if self.filtered() => {
                self.filter.clear();
                self.show = Show::All;
                self.after_view_change();
            }
            // Esc never closes the panel: it only steps back.
            KeyCode::Esc => {}
            KeyCode::Char('q') => self.ask_quit = true,
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => self.ask_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::Home | KeyCode::Char('g') => self.jump(false),
            KeyCode::End | KeyCode::Char('G') => self.jump(true),
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('h')
            | KeyCode::Char('l') => self.switch_section(),
            KeyCode::Char('1') => self.section = Section::Review,
            KeyCode::Char('2') => self.section = Section::Mine,
            KeyCode::Enter | KeyCode::Char('o') => self.open_selected(),
            KeyCode::Char('y') => self.copy_selected(),
            KeyCode::Char('r') => {
                self.forget_failed_activity();
                if self.loading {
                    self.say("Already refreshing");
                } else {
                    self.refresh();
                }
            }
            KeyCode::Char('a') => self.toggle_scope(),
            KeyCode::Char('i') => self.toggle_icons(),
            _ => {}
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        self.refresh();
        while !self.quit {
            self.receive();
            self.receive_actions();
            self.receive_activity();
            self.receive_diff();
            self.receive_resolved();
            self.load_activity();
            if !self.loading && Instant::now() >= self.next_refresh {
                self.refresh();
            }
            if let Some((_, until)) = &self.flash {
                if Instant::now() >= *until {
                    self.flash = None;
                }
            }
            terminal.draw(|frame| ui::draw(frame, &mut self))?;
            if event::poll(Duration::from_millis(250))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => self.key(k.code, k.modifiers),
                    Event::Mouse(m) => self.mouse(m),
                    _ => {}
                }
            }
        }
        Ok(())
    }
}
