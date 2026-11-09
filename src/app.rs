//! The list's state and its event loop: keys, mouse, approvals, automatic
//! and manual refresh.

use crate::compose::{Compose, Edit, Stage, Target};
use crate::config::{Config, Scope};
use crate::forge::{self, Fetched, Posted};
use crate::model::{AllList, Data, Forge, MergeRequest, MergeState, Status};
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
    Mine,
    Review,
    /// Every open merge request of the project, or of your projects.
    All,
}

impl Section {
    /// The sections in the order of their tabs; the list opens on the first,
    /// and the number keys pick them by position (`1` is the first).
    pub const ORDER: [Section; 3] = [Section::Mine, Section::Review, Section::All];

    /// The name on its tab.
    pub fn name(self) -> &'static str {
        match self {
            Section::Mine => "Mine",
            Section::Review => "To review",
            Section::All => "All",
        }
    }

    /// The next tab (`forward`) or the previous one, round at the ends.
    pub fn step(self, forward: bool) -> Section {
        let n = Section::ORDER.len();
        let i = Section::ORDER.iter().position(|s| *s == self).unwrap_or(0);
        Section::ORDER[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }]
    }

    /// The section a number key picks, by its position in [`Section::ORDER`].
    pub fn numbered(key: char) -> Option<Section> {
        let index = key.to_digit(10)?.checked_sub(1)?;
        Section::ORDER.get(index as usize).copied()
    }
}

/// Something on screen a mouse click or the wheel acts on, recorded while
/// drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Section(Section),
    Scope,
    /// The title: steps open, merged, closed.
    Status,
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
    /// A link in the details panel or the help's border (index into
    /// `App::links`).
    Link(usize),
    /// Anywhere while the help is open: closes it.
    Help,
    /// A button of the text field in the footer, by its key (`s` for
    /// `Ctrl+s`, `\u{1b}` for `Esc`); the only clicks while it is open.
    Compose(char),
    /// The text field's box (the wheel scrolls it).
    ComposeBox,
}

/// Sends a text to the forge: the list's own is [`post`]; tests put in one
/// that never runs glab or gh.
pub type Poster = fn(&MergeRequest, &Target, &str) -> Result<Posted, String>;

/// Runs the system's opener (first argument) on a web link, true when it
/// started; tests put in one that starts nothing.
pub type Opener = fn(&str, &str) -> bool;

/// Starts `opener` (`open`, `xdg-open`) on `url`.
pub fn open_in_browser(opener: &str, url: &str) -> bool {
    Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

/// Posts a reply, a comment or a change request through the merge
/// request's forge.
pub fn post(mr: &MergeRequest, target: &Target, body: &str) -> Result<Posted, String> {
    match target {
        Target::Reply { thread, .. } => forge::reply(mr, thread, body),
        Target::Comment => forge::comment(mr, body).map(|()| Posted::Now),
        Target::RequestChanges => forge::request_changes(mr, body).map(|()| Posted::Now),
    }
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

/// A load of comments as it comes back: for which merge request, which
/// load (see `App::activity_wanted`) and its answer.
type ActivityAnswer = (
    ActivityKey,
    u64,
    Result<Vec<crate::model::Discussion>, String>,
);

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

/// The lists of a state that is not on screen, kept so switching back shows
/// them at once.
#[derive(Debug, Default)]
struct Parked {
    data: Option<Data>,
    error: Option<String>,
    updated_at: Option<Instant>,
    /// Where each section was, so coming back shows it as it was left.
    tables: Option<[TableState; 3]>,
}

impl Parked {
    /// Keeps a refresh's answer.
    fn store(&mut self, result: Result<Fetched, String>) {
        match result {
            Ok(fetched) => {
                self.data = Some(fetched.data);
                self.error = fetched.warning;
                self.updated_at = Some(Instant::now());
            }
            Err(e) => self.error = Some(e),
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
    /// Open, merged or closed ones on screen; open every time the list opens.
    pub status: Status,
    /// The lists of the other two states, as far as loaded.
    parked: std::collections::HashMap<Status, Parked>,
    pub review_state: TableState,
    pub mine_state: TableState,
    pub all_state: TableState,
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
    /// The newest load asked for per merge request; answers of older ones
    /// are dropped, so a load started before a post can't hide the post.
    activity_wanted: std::collections::HashMap<ActivityKey, u64>,
    activity_seq: u64,
    /// The thread (by the forge's id) to select again once the comments of
    /// that merge request have loaded again: they are sorted newest first,
    /// so a thread moves when someone replies.
    keep_thread: Option<(ActivityKey, String)>,
    activity_tx: Sender<ActivityAnswer>,
    resolve_tx: Sender<(ActivityKey, usize, bool, Result<(), String>)>,
    resolve_rx: Receiver<(ActivityKey, usize, bool, Result<(), String>)>,
    /// Scroll the activity tab to the selected thread on the next frame.
    pub scroll_to_thread: bool,
    activity_rx: Receiver<ActivityAnswer>,
    /// A reply or comment being written; while open, every key goes into it.
    pub compose: Option<Compose>,
    pub poster: Poster,
    pub opener: Opener,
    post_tx: Sender<Result<Posted, String>>,
    post_rx: Receiver<Result<Posted, String>>,
    /// Refresh again as soon as the one running now has answered.
    refresh_again: bool,
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
    /// The folder the list runs in; `b` fetches into its clone.
    pub workdir: Option<std::path::PathBuf>,
    /// The remotes of that clone with the projects they point at (`origin`
    /// and, say, `upstream`).
    pub remotes: Vec<crate::repo::Remote>,
    /// The list runs inside Herdr, which `b` needs to open a workspace.
    pub herdr: bool,
    /// `b` is fetching and opening a branch.
    pub opening_branch: bool,
    branch_tx: Sender<Result<String, String>>,
    branch_rx: Receiver<Result<String, String>>,
    tx: Sender<(Status, Result<Fetched, String>)>,
    rx: Receiver<(Status, Result<Fetched, String>)>,
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
        let (branch_tx, branch_rx) = mpsc::channel();
        let (post_tx, post_rx) = mpsc::channel();
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
            section: Section::ORDER[0],
            status: Status::Open,
            parked: std::collections::HashMap::new(),
            review_state: TableState::default(),
            mine_state: TableState::default(),
            all_state: TableState::default(),
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
            activity_wanted: std::collections::HashMap::new(),
            activity_seq: 0,
            keep_thread: None,
            activity_tx,
            activity_rx,
            compose: None,
            poster: post,
            opener: open_in_browser,
            post_tx,
            post_rx,
            refresh_again: false,
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
            workdir: None,
            remotes: Vec::new(),
            herdr: std::env::var("HERDR_ENV").is_ok_and(|v| v == "1"),
            opening_branch: false,
            branch_tx,
            branch_rx,
            tx,
            rx,
            quit: false,
        }
    }

    /// "Pull requests" when everything in view is on GitHub, else "Merge
    /// requests" (GitLab's word, also for a mix of both).
    pub fn plural_noun(&self) -> &'static str {
        let github = match self.scoped_project() {
            Some(p) => p.on(crate::github::HOST),
            None => self.data.as_ref().is_some_and(|d| {
                let mut all = d
                    .review
                    .iter()
                    .chain(&d.mine)
                    .chain(self.all_items())
                    .peekable();
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
                    .chain(&d.mine)
                    .chain(self.all_items())
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

    /// "All" as loaded for the scope shown now; `None` while the list for
    /// this scope is still loading (the first time, or after `a`).
    pub fn all_list(&self) -> Option<&AllList> {
        self.data
            .as_ref()?
            .all
            .as_ref()
            .filter(|all| all.of.as_ref() == self.scoped_project())
    }

    /// Whether you review this merge request: you are among its reviewers,
    /// or it is in "To review" (on GitHub also through a team you are in).
    pub fn reviews(&self, mr: &MergeRequest) -> bool {
        mr.review_of(&mr.viewer).is_some()
            || self.data.as_ref().is_some_and(|d| {
                d.review
                    .iter()
                    .any(|r| r.iid == mr.iid && r.project == mr.project && r.host == mr.host)
            })
    }

    /// Whether "Mine" and "To review" as loaded fit the scope shown now:
    /// open ones always (the list filters them), merged and closed ones only
    /// when they were asked for this scope.
    pub fn lists_fit(&self) -> bool {
        self.data
            .as_ref()
            .is_some_and(|d| self.status == Status::Open || d.of.as_ref() == self.scoped_project())
    }

    /// Whether a section still waits for its list: nothing loaded yet, or
    /// loaded for the other scope.
    pub fn loading_section(&self, section: Section) -> bool {
        match section {
            Section::All => self.all_list().is_none(),
            _ => !self.lists_fit(),
        }
    }

    fn all_items(&self) -> impl Iterator<Item = &MergeRequest> {
        self.all_list().into_iter().flat_map(|all| &all.items)
    }

    /// The merge requests of a section as shown: scope, quick filter and
    /// typed filter applied, then sorted.
    pub fn rows(&self, section: Section) -> Vec<&MergeRequest> {
        let Some(data) = &self.data else {
            return Vec::new();
        };
        if self.loading_section(section) {
            return Vec::new();
        }
        let list: Vec<&MergeRequest> = match section {
            Section::Review => data.review.iter().collect(),
            Section::Mine => data.mine.iter().collect(),
            // Loaded for this scope already, so no project to filter by.
            Section::All => self.all_items().collect(),
        };
        let mut rows: Vec<&MergeRequest> = list
            .into_iter()
            .filter(|mr| match self.scoped_project() {
                Some(p) if section != Section::All => p.is(&mr.host, &mr.project),
                _ => true,
            })
            .filter(|mr| match section {
                Section::Review => self.show.keeps(mr, &mr.viewer, true),
                Section::Mine => self.show.keeps(mr, &mr.viewer, false),
                Section::All => self.show.keeps_in_all(mr, &mr.viewer, self.reviews(mr)),
            })
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
        for section in Section::ORDER {
            let state = self.table_state_mut(section);
            state.select(Some(0));
            *state.offset_mut() = 0;
        }
        self.details_scroll = 0;
        self.clamp_selection();
    }

    /// Where a section's list is scrolled to and which row is selected.
    pub fn table_state(&self, section: Section) -> &TableState {
        match section {
            Section::Review => &self.review_state,
            Section::Mine => &self.mine_state,
            Section::All => &self.all_state,
        }
    }

    pub fn table_state_mut(&mut self, section: Section) -> &mut TableState {
        match section {
            Section::Review => &mut self.review_state,
            Section::Mine => &mut self.mine_state,
            Section::All => &mut self.all_state,
        }
    }

    pub fn state_mut(&mut self) -> &mut TableState {
        self.table_state_mut(self.section)
    }

    pub fn selected(&self) -> Option<&MergeRequest> {
        let rows = self.rows(self.section);
        self.table_state(self.section)
            .selected()
            .and_then(|i| rows.get(i).copied())
    }

    fn refresh(&mut self) {
        if self.loading {
            return;
        }
        self.loading = true;
        let tx = self.tx.clone();
        let host = self.config.gitlab_host.clone();
        let (status, all_of) = self.fetch_request();
        std::thread::spawn(move || {
            let _ = tx.send((status, forge::fetch_all(&host, status, all_of.as_ref())));
        });
    }

    /// Refreshes from the main loop at once, or right after the refresh that
    /// is running now, which may have been asked before a write.
    fn refresh_soon(&mut self) {
        self.next_refresh = Instant::now();
        if self.loading {
            self.refresh_again = true;
        }
    }

    /// What a refresh asks for: the state on screen, and "All" (and merged
    /// or closed lists) for the scope on screen.
    pub fn fetch_request(&self) -> (Status, Option<Project>) {
        (self.status, self.scoped_project().cloned())
    }

    /// Steps the list to the next state: open, merged, closed. The lists of
    /// the state left are kept, so coming back shows them at once.
    fn step_status(&mut self) {
        let next = self.status.next();
        let shown = Parked {
            data: self.data.take(),
            error: self.error.take(),
            updated_at: self.updated_at.take(),
            tables: Some(Section::ORDER.map(|s| *self.table_state(s))),
        };
        self.parked.insert(self.status, shown);
        let back = self.parked.remove(&next).unwrap_or_default();
        self.data = back.data;
        self.error = back.error;
        self.updated_at = back.updated_at;
        self.status = next;
        // The quick filters are for open ones.
        if next != Status::Open {
            self.show = Show::All;
        }
        match back.tables {
            Some(tables) => {
                for (section, table) in Section::ORDER.into_iter().zip(tables) {
                    *self.table_state_mut(section) = table;
                }
                self.clamp_selection();
            }
            None => self.after_view_change(),
        }
        self.details_scroll = 0;
        // Other merge requests now: a thread chosen before is not theirs.
        self.thread = None;
        self.schedule_shown();
    }

    /// Whether any section still waits for its list for this scope.
    fn any_section_loading(&self) -> bool {
        Section::ORDER.iter().any(|s| self.loading_section(*s))
    }

    /// When to load the state on screen: at once when its lists are
    /// missing, for the other scope or old (after the refresh running now,
    /// if any), otherwise one interval after they were loaded.
    fn schedule_shown(&mut self) {
        let every = Duration::from_secs(self.config.refresh_secs);
        self.next_refresh = match self.updated_at {
            Some(at) if at.elapsed() < every && !self.any_section_loading() => at + every,
            _ => Instant::now(),
        };
    }

    /// After a write to a forge: the kept merged and closed lists may miss
    /// what it changed (a merge), so they load again when shown.
    fn after_write(&mut self) {
        for parked in self.parked.values_mut() {
            parked.updated_at = None;
        }
    }

    /// Says so when `mr` is merged or closed, where only open ones can be
    /// approved, merged, set to draft, opened as a branch, replied to,
    /// commented on or sent a change request.
    fn refuse_closed(&mut self, mr: &MergeRequest) -> bool {
        if mr.status == Status::Open {
            return false;
        }
        self.say(format!("{} is {}", mr.reference(), mr.status.name()));
        true
    }

    fn receive_actions(&mut self) {
        while let Ok(result) = self.act_rx.try_recv() {
            match result {
                Ok(msg) => {
                    self.say(msg);
                    self.after_write();
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
        if self.refuse_closed(&mr) {
            return;
        }
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
        if self.refuse_closed(&mr) {
            return;
        }
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
        if self.refuse_closed(&mr) {
            return;
        }
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

    /// The remote of the clone the list runs in that points at the merge
    /// request's project, so `b` can fetch its branch from there.
    pub fn remote_of(&self, mr: &MergeRequest) -> Option<&str> {
        self.workdir.as_ref()?;
        self.remotes
            .iter()
            .find(|r| r.points_at(&mr.host, &mr.project))
            .map(|r| r.name.as_str())
    }

    /// Fetches the selected merge request's branch and opens it in a new
    /// Herdr workspace, in the background. No confirmation: it writes
    /// nothing to the forge and leaves the current checkout alone.
    fn open_branch(&mut self) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        if self.refuse_closed(&mr) {
            return;
        }
        if self.opening_branch {
            self.say("Still opening a branch");
            return;
        }
        if !self.herdr {
            self.say("Opening the branch only works inside Herdr");
            return;
        }
        let (Some(dir), Some(remote)) = (
            self.workdir.clone(),
            self.remote_of(&mr).map(str::to_string),
        ) else {
            self.say(format!(
                "No local clone of {} here: open the list in its folder",
                mr.project
            ));
            return;
        };
        let branch = crate::branch::local_branch(&mr);
        if !crate::branch::valid_branch(&branch) {
            self.say(format!(
                "Cannot open the branch {branch:?}: git does not allow that name"
            ));
            return;
        }
        self.opening_branch = true;
        // Shown until the answer comes: the fetch has 30 s, Herdr two minutes.
        self.flash = Some((
            format!("Fetching {branch}…"),
            Instant::now() + Duration::from_secs(180),
        ));
        let tx = self.branch_tx.clone();
        std::thread::spawn(move || {
            // A panic still answers, so `b` does not stay blocked.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::branch::open(&mr, &dir, &remote, &crate::branch::run_herdr)
            }))
            .unwrap_or_else(|_| Err(format!("Opening {branch} failed")));
            let _ = tx.send(result);
        });
    }

    fn receive_branch(&mut self) {
        while let Ok(result) = self.branch_rx.try_recv() {
            self.opening_branch = false;
            match result {
                Ok(msg) | Err(msg) => self.say(msg),
            }
        }
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
        // The selected thread is found again by its id once loaded.
        if let (Some(i), Some((_, Activity::Loaded(list)))) = (self.thread, self.activity.get(&key))
        {
            if let Some(d) = list.get(i) {
                // A thread kept by a post for this merge request wins.
                if self.keep_thread.as_ref().is_none_or(|(k, _)| *k != key) {
                    self.keep_thread = Some((key.clone(), d.id.clone()));
                }
            }
        }
        self.activity
            .insert(key.clone(), (updated, Activity::Loading));
        self.activity_seq += 1;
        let seq = self.activity_seq;
        self.activity_wanted.insert(key.clone(), seq);
        let tx = self.activity_tx.clone();
        std::thread::spawn(move || {
            let result = forge::fetch_activity(&mr);
            let _ = tx.send((key, seq, result));
        });
    }

    /// Comments that failed to load are tried again the next time they show.
    fn forget_failed_activity(&mut self) {
        self.activity
            .retain(|_, (_, a)| !matches!(a, Activity::Failed(_)));
    }

    fn receive_activity(&mut self) {
        while let Ok((key, seq, result)) = self.activity_rx.try_recv() {
            if self.activity_wanted.get(&key) != Some(&seq) {
                // An older load, or one from before a post.
                continue;
            }
            let updated = self.activity.get(&key).and_then(|(u, _)| *u);
            let value = match result {
                Ok(list) => {
                    self.reselect_thread(&key, &list);
                    Activity::Loaded(list)
                }
                Err(e) => Activity::Failed(e),
            };
            self.activity.insert(key, (updated, value));
        }
    }

    /// Selects the kept thread again in the comments just loaded for `key`,
    /// when that merge request is still the selected one.
    fn reselect_thread(&mut self, key: &ActivityKey, list: &[crate::model::Discussion]) {
        let Some((kept, id)) = self.keep_thread.clone() else {
            return;
        };
        if &kept != key {
            return;
        }
        self.keep_thread = None;
        if self.selected().map(|mr| mr.key()).as_ref() != Some(key) {
            return;
        }
        // Only when the filter shows it; otherwise `e` or `>` would act on
        // a thread nobody sees.
        self.thread = list
            .iter()
            .position(|d| d.id == id && d.resolvable && self.thread_filter.keeps(d));
        self.scroll_to_thread = self.thread.is_some();
    }

    /// The selected thread, when it is one that takes replies.
    fn selected_thread(&self) -> Option<&crate::model::Discussion> {
        match self.selected_activity() {
            Some(Activity::Loaded(list)) => self
                .thread
                .and_then(|i| list.get(i))
                .filter(|d| d.resolvable && !d.id.is_empty()),
            _ => None,
        }
    }

    /// The forge's id of the selected thread, when it belongs to `key`.
    fn selected_thread_id(&self, key: &ActivityKey) -> Option<String> {
        if self.selected().map(|mr| mr.key()).as_ref() != Some(key) {
            return None;
        }
        self.selected_thread().map(|d| d.id.clone())
    }

    /// `>`: opens the text field for a reply to the selected thread.
    fn open_reply(&mut self) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        if self.refuse_closed(&mr) {
            return;
        }
        let thread = self
            .selected_thread()
            .map(|d| (d.id.clone(), d.place.clone()));
        let Some((thread, place)) = thread else {
            self.say("Choose a thread first: n");
            return;
        };
        self.compose = Some(Compose::new(mr, Target::Reply { thread, place }));
    }

    /// `!`: opens the text field for the reason to request changes on the
    /// selected merge request; like approving, not on your own.
    fn open_request_changes(&mut self) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        if self.refuse_closed(&mr) {
            return;
        }
        if mr.author == mr.viewer {
            self.say(format!(
                "You cannot request changes on your own {}",
                mr.forge.noun()
            ));
            return;
        }
        self.compose = Some(Compose::new(mr, Target::RequestChanges));
    }

    /// `+`: opens the text field for a comment on the selected merge request.
    fn open_comment(&mut self) {
        let Some(mr) = self.selected().cloned() else {
            return;
        };
        if self.refuse_closed(&mr) {
            return;
        }
        self.compose = Some(Compose::new(mr, Target::Comment));
    }

    /// Keys while the text field is open: they all go to it.
    fn compose_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        // A text taller than the box scrolls, also in the preview, so all
        // of it can be read before it is posted.
        match code {
            KeyCode::Up => return c.scroll(1),
            KeyCode::Down => return c.scroll(-1),
            KeyCode::PageUp => return c.scroll(5),
            KeyCode::PageDown => return c.scroll(-5),
            _ => {}
        }
        match c.stage {
            Stage::Writing => match c.field.key(code, mods) {
                Edit::Changed => {
                    c.message = None;
                    // Back to the end, where the typing is.
                    c.scroll_up = 0;
                }
                Edit::Finish if c.field.is_blank() => {
                    c.message = Some(c.blank().into());
                }
                Edit::Finish => {
                    c.message = None;
                    c.stage = Stage::Preview;
                    // The preview starts at the top of the text.
                    c.scroll_up = u16::MAX;
                }
                Edit::Discard => self.discard_compose(),
                Edit::Ignored => {}
            },
            // Only y posts: Enter could be pressed once too often.
            Stage::Preview => match code {
                KeyCode::Char('y') if !mods.contains(KeyModifiers::CONTROL) => self.post(),
                KeyCode::Char('n') if !mods.contains(KeyModifiers::CONTROL) => {
                    c.stage = Stage::Writing
                }
                KeyCode::Esc => self.discard_compose(),
                _ => {}
            },
            // Sent: wait for the forge.
            Stage::Posting => {}
        }
    }

    /// A click on one of the text field's buttons in the footer.
    fn compose_click(&mut self, key: char) {
        let Some(stage) = self.compose.as_ref().map(|c| c.stage) else {
            return;
        };
        match (stage, key) {
            (Stage::Writing, 's') => self.compose_key(KeyCode::Char('s'), KeyModifiers::CONTROL),
            (Stage::Preview, 'y' | 'n') => self.compose_key(KeyCode::Char(key), KeyModifiers::NONE),
            (Stage::Writing | Stage::Preview, '\u{1b}') => {
                self.compose_key(KeyCode::Esc, KeyModifiers::NONE)
            }
            _ => {}
        }
    }

    fn discard_compose(&mut self) {
        let noun = self.compose.take().map(|c| c.noun()).unwrap_or("text");
        self.say(format!("Discarded the {noun}"));
    }

    /// Sends the text in the background; the field stays until the forge
    /// answers, so a failure keeps the text.
    fn post(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        c.stage = Stage::Posting;
        c.message = None;
        let job = c.clone();
        let tx = self.post_tx.clone();
        let poster = self.poster;
        std::thread::spawn(move || {
            // A panic still answers, so the field does not wait for ever.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                poster(&job.mr, &job.target, &job.field.text)
            }))
            .unwrap_or_else(|_| Err(format!("Posting the {} failed", job.noun())));
            let _ = tx.send(result);
        });
    }

    fn receive_posted(&mut self) {
        while let Ok(result) = self.post_rx.try_recv() {
            let Some(mut c) = self.compose.take() else {
                continue;
            };
            match result {
                Ok(posted) => {
                    self.say(match posted {
                        Posted::Now => c.done(),
                        Posted::InPendingReview => {
                            "Reply added to your pending review: submit it on GitHub".to_string()
                        }
                    });
                    let key = c.mr.key();
                    let keep = match &c.target {
                        Target::Reply { thread, .. } => Some(thread.clone()),
                        Target::Comment | Target::RequestChanges => {
                            // New comments are at the top.
                            if self.selected().map(|mr| mr.key()).as_ref() == Some(&key) {
                                self.details_scroll = 0;
                            }
                            self.selected_thread_id(&key)
                        }
                    };
                    self.keep_thread = keep.map(|id| (key.clone(), id));
                    // Load its comments again, and drop any load still running.
                    self.activity.remove(&key);
                    self.activity_wanted.remove(&key);
                    self.after_write();
                    self.refresh_soon();
                }
                Err(e) => {
                    // Out of time: the forge may have taken it all the same,
                    // so load its comments again to see.
                    let e = if e.contains("did not answer within") {
                        let key = c.mr.key();
                        self.activity.remove(&key);
                        self.activity_wanted.remove(&key);
                        format!(
                            "{e}: the {} may have been {}, check above before sending again",
                            c.noun(),
                            c.past()
                        )
                    } else {
                        e
                    };
                    c.stage = Stage::Writing;
                    c.message = Some(e.clone());
                    self.compose = Some(c);
                    self.say(e);
                }
            }
        }
    }

    /// Pasted text (bracketed paste): whole into the text field or the
    /// search, so a line break or a `y` in it acts on nothing else.
    fn paste(&mut self, text: &str) {
        if let Some(c) = self.compose.as_mut() {
            if c.stage == Stage::Writing {
                c.field.paste(text);
                c.message = None;
                c.scroll_up = 0;
            }
            return;
        }
        if self.typing {
            let line: String = text
                .replace("\r\n", "\n")
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            self.filter.push_str(&line);
            self.after_view_change();
        }
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind == KeyEventKind::Press => self.key(k.code, k.modifiers),
            Event::Mouse(m) => self.mouse(m),
            Event::Paste(text) => self.paste(&text),
            _ => {}
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
            if let MouseEventKind::Down(button) = m.kind {
                self.help = false;
                // The version in the border opens the repository.
                if let (MouseButton::Left, Some(Hit::Link(i))) = (button, hit) {
                    if let Some(url) = self.links.get(i).cloned() {
                        self.open_url(&url);
                    }
                }
            }
            return;
        }
        // While writing, clicks only reach the field's own buttons, and the
        // wheel only scrolls the panel: nothing may change what the text is
        // for or type into it.
        if self.compose.is_some() {
            let over_panel = matches!(
                hit,
                Some(Hit::Details | Hit::Thread(_) | Hit::Resolve(_) | Hit::Threads(_))
            );
            match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(Hit::Compose(c)) = hit {
                        self.compose_click(c);
                    }
                }
                MouseEventKind::ScrollDown if hit == Some(Hit::ComposeBox) => {
                    self.compose_key(KeyCode::Down, KeyModifiers::NONE)
                }
                MouseEventKind::ScrollUp if hit == Some(Hit::ComposeBox) => {
                    self.compose_key(KeyCode::Up, KeyModifiers::NONE)
                }
                MouseEventKind::ScrollDown if over_panel => {
                    self.details_scroll = self.details_scroll.saturating_add(2)
                }
                MouseEventKind::ScrollUp if over_panel => {
                    self.details_scroll = self.details_scroll.saturating_sub(2)
                }
                _ => {}
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
                Some(Hit::Status) => self.step_status(),
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
        while let Ok((status, result)) = self.rx.try_recv() {
            self.loading = false;
            self.next_refresh = Instant::now() + Duration::from_secs(self.config.refresh_secs);
            if status != self.status {
                // Asked for before the state changed: keep it for that state,
                // and load the one on screen when it needs it.
                self.parked.entry(status).or_default().store(result);
                self.schedule_shown();
                continue;
            }
            if self.refresh_again {
                // Asked for before a write: load again with it.
                self.refresh_again = false;
                self.next_refresh = Instant::now();
            }
            match result {
                Ok(fetched) => {
                    // Each section keeps its merge request selected, also
                    // when it moved (the newest come first, and a write
                    // makes it the newest).
                    let kept = Section::ORDER.map(|s| self.selected_key_in(s));
                    let shown = self.selected().map(|mr| mr.key());
                    self.data = Some(fetched.data);
                    self.error = fetched.warning;
                    self.updated_at = Some(Instant::now());
                    for (section, key) in Section::ORDER.into_iter().zip(kept) {
                        let Some(key) = key else {
                            continue;
                        };
                        if let Some(i) = self.rows(section).iter().position(|mr| mr.key() == key) {
                            self.table_state_mut(section).select(Some(i));
                        }
                    }
                    self.clamp_selection();
                    if self.selected().map(|mr| mr.key()) != shown {
                        // Another merge request now: the thread is not its.
                        self.thread = None;
                    }
                    // `a` was pressed while loading: "All" (or merged and
                    // closed lists) are for the other scope. Only after an
                    // answer, so a failing forge is not asked again at once,
                    // over and over.
                    if self.any_section_loading() {
                        self.next_refresh = Instant::now();
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// The merge request selected in a section, by its key.
    fn selected_key_in(&self, section: Section) -> Option<ActivityKey> {
        let i = self.table_state(section).selected()?;
        self.rows(section).get(i).map(|mr| mr.key())
    }

    /// Keeps each section's selection inside its list.
    fn clamp_selection(&mut self) {
        for section in Section::ORDER {
            let len = self.rows(section).len();
            let state = self.table_state_mut(section);
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

    fn switch_section(&mut self, forward: bool) {
        self.section = self.section.step(forward);
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
        if !crate::text::is_web_link(url) {
            self.say("Only web links can be opened");
            return;
        }
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        self.say(if (self.opener)(opener, url) {
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
        for section in Section::ORDER {
            self.table_state_mut(section).select(Some(0));
        }
        self.thread = None;
        self.clamp_selection();
        // "All", and merged and closed ones, list other merge requests per
        // scope: load them now (the main loop starts it; one already running
        // is followed by another).
        if self.any_section_loading() {
            self.next_refresh = Instant::now();
        }
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
        // While a reply or comment is written, every key goes into it.
        if self.compose.is_some() {
            self.compose_key(code, mods);
            return;
        }
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
            KeyCode::Char('>') if self.in_activity() => self.open_reply(),
            KeyCode::Char('+') if self.in_activity() => self.open_comment(),
            KeyCode::Char('e') if self.in_activity() => {
                if let Some(t) = self.thread {
                    self.toggle_resolved(t);
                } else {
                    self.say("Choose a thread first: n");
                }
            }
            KeyCode::Char('v') => self.ask_approval(),
            KeyCode::Char('!') => self.open_request_changes(),
            KeyCode::Char('m') => self.ask_merge(),
            KeyCode::Char('w') => self.toggle_draft(),
            KeyCode::Char('b') => self.open_branch(),
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
            KeyCode::Char('f') if self.status != Status::Open => {
                let noun = self.plural_noun().to_lowercase();
                self.say(format!("Quick filters are for open {noun}"));
            }
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
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => self.switch_section(true),
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => self.switch_section(false),
            KeyCode::Char(c @ '1'..='9') => {
                if let Some(section) = Section::numbered(c) {
                    self.section = section;
                }
            }
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
            KeyCode::Char('z') => self.step_status(),
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
            self.receive_branch();
            self.receive_posted();
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
                self.event(event::read()?);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Pipeline, ReviewState};
    use crate::view::SortKey;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// A made-up merge request.
    fn sample_mr(iid: u64) -> MergeRequest {
        MergeRequest {
            forge: Forge::GitLab,
            host: "gitlab.com".into(),
            viewer: "me".into(),
            iid,
            title: format!("Change {iid}"),
            description: String::new(),
            url: String::new(),
            project: "acme/app".into(),
            author: "alex".into(),
            source_branch: format!("change-{iid}"),
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
            reviewers: vec![crate::model::Reviewer {
                username: "me".into(),
                state: ReviewState::Unreviewed,
            }],
            threads_total: 0,
            threads_open: 0,
            labels: vec![],
            pipeline: Pipeline::None,
            additions: Some(1),
            deletions: Some(1),
            merge_method: String::new(),
        }
    }

    /// An open list with three merge requests in each section.
    fn app() -> App {
        let config = Config {
            icons_chosen: true,
            ..Config::default()
        };
        let mut app = App::new(config, None, None);
        // Not from the shell the tests run in.
        app.herdr = false;
        app.data = Some(Data {
            review: (1..=3).map(sample_mr).collect(),
            mine: (4..=6).map(sample_mr).collect(),
            of: None,
            all: Some(AllList {
                of: None,
                items: all_rows(),
                total: 4,
                newest: 4,
                error: None,
            }),
        });
        app.clamp_selection();
        app
    }

    /// "All" of `acme/app`, newest first: someone else's that I do not
    /// review (8), mine (7), one I review (1) and one I approved (2).
    fn all_rows() -> Vec<MergeRequest> {
        let mut other = sample_mr(8);
        other.author = "sam".into();
        other.reviewers.clear();
        let mut own = sample_mr(7);
        own.author = "me".into();
        own.reviewers.clear();
        own.threads_open = 1;
        let mut approved = sample_mr(2);
        approved.reviewers[0].state = ReviewState::Approved;
        approved.approved_by = vec!["me".into()];
        vec![other, own, approved, sample_mr(1)]
    }

    fn iids(rows: &[&MergeRequest]) -> Vec<u64> {
        rows.iter().map(|mr| mr.iid).collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.key(code, KeyModifiers::NONE);
    }

    fn click(app: &mut App, x: u16, y: u16) {
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
    }

    /// The list opened in a clone of `acme/app` on gitlab.com, inside Herdr.
    fn app_in_clone(dir: &std::path::Path) -> App {
        let mut app = app_in("gitlab.com", "acme/app");
        app.remotes = vec![crate::repo::Remote {
            name: "origin".into(),
            project: app.project.clone().unwrap(),
        }];
        app.workdir = Some(dir.to_path_buf());
        app.herdr = true;
        app
    }

    fn flash(app: &App) -> Option<&str> {
        app.flash.as_ref().map(|(t, _)| t.as_str())
    }

    /// `app()` opened in a clone whose remote names `host`/`path`.
    fn app_in(host: &str, path: &str) -> App {
        let mut app = app();
        app.project = Some(Project {
            host: host.into(),
            path: path.into(),
        });
        app.scope = Scope::Current;
        app.clamp_selection();
        app
    }

    #[test]
    fn this_project_ignores_the_case_of_the_remote() {
        // The forge says `acme/app` on `gitlab.com`; the clone was made from
        // other capitals.
        for (host, path) in [
            ("gitlab.com", "Acme/App"),
            ("GitLab.com", "acme/app"),
            ("GITLAB.COM", "ACME/APP"),
        ] {
            let app = app_in(host, path);
            assert_eq!(iids(&app.rows(Section::Mine)), [6, 5, 4], "{host}/{path}");
            assert_eq!(iids(&app.rows(Section::Review)), [3, 2, 1], "{host}/{path}");
        }
        // A project whose name only starts or ends the same is another one.
        for path in ["acme/app2", "acme/ap", "Acme/App2", "other/acme/app"] {
            let app = app_in("gitlab.com", path);
            assert!(iids(&app.rows(Section::Mine)).is_empty(), "{path}");
            assert!(iids(&app.rows(Section::Review)).is_empty(), "{path}");
        }
    }

    #[test]
    fn the_header_word_follows_a_project_in_other_case() {
        let mut app = app_in("GitHub.com", "Acme/App");
        assert_eq!(app.plural_noun(), "Pull requests");
        app.project.as_mut().unwrap().host = "GitLab.com".into();
        assert_eq!(app.plural_noun(), "Merge requests");
    }

    #[test]
    fn b_refuses_rows_of_other_projects() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        for mr in &mut app.data.as_mut().unwrap().mine {
            mr.project = "acme/other".into();
        }
        // Other projects only show with "all projects".
        app.scope = Scope::All;
        app.clamp_selection();
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(
            flash(&app),
            Some("No local clone of acme/other here: open the list in its folder")
        );
        assert!(!app.opening_branch);
        // Without a project of its own the list has no clone at all.
        let mut outside = self::app();
        outside.herdr = true;
        press(&mut outside, KeyCode::Char('b'));
        assert!(flash(&outside)
            .unwrap()
            .starts_with("No local clone of acme/app"));
        assert!(!outside.opening_branch);
    }

    #[test]
    fn b_only_works_inside_herdr() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        app.herdr = false;
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(
            flash(&app),
            Some("Opening the branch only works inside Herdr")
        );
        assert!(!app.opening_branch);
    }

    #[test]
    fn b_on_an_empty_list_does_nothing() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        app.data = Some(Data::default());
        app.clamp_selection();
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(flash(&app), None);
        assert!(!app.opening_branch);
    }

    #[test]
    fn b_shows_that_it_fetches_and_the_list_keeps_working() {
        // A folder that is no clone: the background run fails quickly.
        let dir = std::env::temp_dir().join(format!("herdr-git-panel-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = app_in_clone(&dir);
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(flash(&app), Some("Fetching change-6…"));
        assert!(app.opening_branch);
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(flash(&app), Some("Still opening a branch"));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.mine_state.selected(), Some(1), "keys still work");
        // The answer ends the fetching state and shows why it failed.
        let until = Instant::now() + Duration::from_secs(10);
        while app.opening_branch && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
            app.receive_branch();
        }
        assert!(!app.opening_branch);
        let shown = flash(&app).unwrap();
        assert!(shown.starts_with("git worktree failed: "), "{shown}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn number_keys_follow_the_tab_order() {
        let mut app = app();
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.section, Section::Review);
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.section, Section::Mine);
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.section, Section::Review);
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.section, Section::All);
        // Other digits change nothing.
        press(&mut app, KeyCode::Char('4'));
        assert_eq!(app.section, Section::All);
    }

    #[test]
    fn switch_keys_step_through_the_sections_and_wrap() {
        for key in [KeyCode::Tab, KeyCode::Right, KeyCode::Char('l')] {
            let mut app = app();
            assert_eq!(app.section, Section::Mine);
            let mut seen = Vec::new();
            for _ in 0..3 {
                press(&mut app, key);
                seen.push(app.section);
            }
            assert_eq!(
                seen,
                [Section::Review, Section::All, Section::Mine],
                "{key:?}"
            );
        }
        for key in [KeyCode::BackTab, KeyCode::Left, KeyCode::Char('h')] {
            let mut app = app();
            let mut seen = Vec::new();
            for _ in 0..3 {
                press(&mut app, key);
                seen.push(app.section);
            }
            assert_eq!(
                seen,
                [Section::All, Section::Review, Section::Mine],
                "{key:?}"
            );
        }
    }

    #[test]
    fn all_lists_everyones_including_mine_and_the_ones_i_review() {
        let app = app();
        assert_eq!(iids(&app.rows(Section::All)), [8, 7, 2, 1]);
    }

    #[test]
    fn all_is_shown_only_for_the_scope_it_was_loaded_for() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        // Loaded for "your projects", shown is "this project".
        assert!(app.all_list().is_none());
        assert!(app.rows(Section::All).is_empty());
        // A refresh is running; `a` asks for the next one at once.
        app.loading = true;
        app.next_refresh = Instant::now() + Duration::from_secs(60);
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.scope, Scope::All);
        assert_eq!(iids(&app.rows(Section::All)), [8, 7, 2, 1]);
        assert!(app.next_refresh > Instant::now(), "loaded for this scope");
        press(&mut app, KeyCode::Char('a'));
        assert!(app.all_list().is_none(), "this project needs its own list");
        assert!(app.next_refresh <= Instant::now(), "refresh at once");
        // The answer for this project arrives.
        let mut data = app.data.clone().unwrap();
        data.all = Some(AllList {
            of: app.project.clone(),
            items: vec![sample_mr(9)],
            total: 1,
            newest: 1,
            error: None,
        });
        app.tx
            .send((
                Status::Open,
                Ok(Fetched {
                    data,
                    warning: None,
                }),
            ))
            .unwrap();
        app.receive();
        assert!(
            app.next_refresh > Instant::now(),
            "no second refresh: it fits the scope"
        );
        assert_eq!(iids(&app.rows(Section::All)), [9]);
    }

    #[test]
    fn an_answer_for_the_old_scope_loads_again_at_once() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        app.scope = Scope::All;
        // A refresh for "your projects" runs when `a` switches to this project.
        app.loading = true;
        press(&mut app, KeyCode::Char('a'));
        let data = app.data.clone().unwrap();
        assert_eq!(data.all.as_ref().unwrap().of, None);
        app.tx
            .send((
                Status::Open,
                Ok(Fetched {
                    data,
                    warning: None,
                }),
            ))
            .unwrap();
        app.receive();
        assert!(!app.loading);
        assert!(
            app.next_refresh <= Instant::now(),
            "the list for this project loads at once"
        );
    }

    #[test]
    fn a_failed_refresh_after_a_does_not_retry_at_once() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        assert!(app.all_list().is_none(), "All is for the other scope");
        app.loading = true;
        app.tx
            .send((Status::Open, Err("glab: no network".into())))
            .unwrap();
        app.receive();
        assert!(
            app.next_refresh > Instant::now(),
            "waits for the next refresh"
        );
        assert_eq!(app.error.as_deref(), Some("glab: no network"));
    }

    #[test]
    fn all_follows_search_filter_sort_and_clear() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        // Search.
        app.filter = "change 7".into();
        assert_eq!(iids(&app.rows(Section::All)), [7]);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(iids(&app.rows(Section::All)), [8, 7, 2, 1]);
        // Quick filter: needs me keeps mine with a thread and the one I
        // have not reviewed yet.
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.show, Show::NeedsMe);
        assert_eq!(iids(&app.rows(Section::All)), [7, 1]);
        press(&mut app, KeyCode::Char('x'));
        // Sort by number, by clicking its heading the same way `s` does.
        app.sort_by(SortKey::Number);
        assert_eq!(iids(&app.rows(Section::All)), [8, 7, 2, 1]);
        app.sort_by(SortKey::Number);
        assert_eq!(iids(&app.rows(Section::All)), [1, 2, 7, 8]);
        // The selection and the details panel act on the row shown.
        app.select(1);
        assert_eq!(app.selected().map(|mr| mr.iid), Some(2));
        press(&mut app, KeyCode::Char('p'));
        assert!(app.details);
    }

    #[test]
    fn actions_in_all_keep_their_rules() {
        let mut app = app();
        press(&mut app, KeyCode::Char('3'));
        // 8 is someone else's: approving asks first, draft is refused.
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(
            flash(&app),
            Some("Only your own merge requests can be set to draft or ready here")
        );
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(
            app.confirm.as_ref().map(|c| c.action),
            Some(Action::Approve)
        );
        press(&mut app, KeyCode::Char('n'));
        assert!(app.confirm.is_none());
        // Merging waits for the forge.
        press(&mut app, KeyCode::Char('m'));
        assert!(flash(&app).unwrap().starts_with("Cannot merge !8"));
        assert!(app.confirm.is_none());
        // 7 is my own: no approval.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(
            flash(&app),
            Some("You cannot approve your own merge request")
        );
        assert!(app.confirm.is_none());
    }

    #[test]
    fn clicking_a_tab_selects_its_section() {
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        // The cell where a tab's label starts, matched cell by cell.
        let find = |label: &str| {
            let letters: Vec<String> = label.chars().map(String::from).collect();
            (0..buf.area.height)
                .find_map(|y| {
                    let cells: Vec<&str> =
                        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                    let x = (0..=cells.len() - letters.len())
                        .find(|&x| letters.iter().zip(&cells[x..]).all(|(l, c)| l == c))?;
                    Some((x as u16 + 1, y))
                })
                .expect(label)
        };
        let (mine_x, mine_y) = find(" Mine 3 ");
        let (review_x, review_y) = find(" To review 3 ");
        let (all_x, all_y) = find(" All 4 ");
        assert!(mine_x < review_x && review_x < all_x);
        click(&mut app, review_x, review_y);
        assert_eq!(app.section, Section::Review);
        click(&mut app, all_x, all_y);
        assert_eq!(app.section, Section::All);
        click(&mut app, mine_x, mine_y);
        assert_eq!(app.section, Section::Mine);
    }

    /// Merged ones in every section, as loaded for `of`.
    fn merged_data(of: Option<Project>) -> Data {
        let merged = |iid| {
            let mut mr = sample_mr(iid);
            mr.status = Status::Merged;
            // What GitHub and GitLab still report for merged ones.
            mr.merge_state = MergeState::Ready;
            mr
        };
        Data {
            review: vec![merged(11), merged(12)],
            mine: vec![merged(13)],
            of: of.clone(),
            all: Some(AllList {
                of,
                items: vec![merged(14)],
                total: 1,
                newest: 1,
                error: None,
            }),
        }
    }

    /// The answer of a refresh for `status`, as the background thread sends it.
    fn answer(app: &mut App, status: Status, data: Data) {
        app.tx
            .send((
                status,
                Ok(Fetched {
                    data,
                    warning: None,
                }),
            ))
            .unwrap();
        app.receive();
    }

    #[test]
    fn the_list_opens_on_open_ones() {
        let app = app();
        assert_eq!(app.status, Status::Open);
        assert_eq!(app.fetch_request(), (Status::Open, None));
    }

    #[test]
    fn z_steps_open_merged_closed_and_back() {
        let mut app = app();
        let mut seen = Vec::new();
        for _ in 0..3 {
            press(&mut app, KeyCode::Char('z'));
            seen.push(app.status);
        }
        assert_eq!(seen, [Status::Merged, Status::Closed, Status::Open]);
        // A refresh asks for the state on screen.
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.fetch_request().0, Status::Merged);
    }

    #[test]
    fn merged_ones_load_on_the_first_switch_and_open_ones_come_back_at_once() {
        let mut app = app();
        app.updated_at = Some(Instant::now());
        app.next_refresh = Instant::now() + Duration::from_secs(60);
        press(&mut app, KeyCode::Char('z'));
        assert!(app.data.is_none(), "nothing merged loaded yet");
        assert!(app.rows(Section::Review).is_empty());
        assert!(app.next_refresh <= Instant::now(), "loads at once");
        answer(&mut app, Status::Merged, merged_data(None));
        assert_eq!(iids(&app.rows(Section::Mine)), [13]);
        assert_eq!(iids(&app.rows(Section::All)), [14]);
        // Back to open: the open lists at once, no waiting.
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.status, Status::Open);
        assert_eq!(iids(&app.rows(Section::Mine)), [6, 5, 4]);
        assert!(
            app.next_refresh > Instant::now(),
            "loaded a moment ago: no refresh needed"
        );
        // And merged again, kept from before.
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(iids(&app.rows(Section::Mine)), [13]);
    }

    #[test]
    fn an_answer_for_a_state_no_longer_shown_is_kept_and_the_shown_one_loads() {
        let mut app = app();
        // A refresh of open ones runs while `z` switches to merged.
        app.loading = true;
        press(&mut app, KeyCode::Char('z'));
        let open = app.parked[&Status::Open].data.clone().unwrap();
        answer(&mut app, Status::Open, open);
        assert!(app.data.is_none(), "not shown as merged ones");
        assert!(app.next_refresh <= Instant::now(), "merged ones load now");
        // Also when that answer was an error.
        app.next_refresh = Instant::now() + Duration::from_secs(60);
        app.loading = true;
        app.tx
            .send((Status::Open, Err("glab: no network".into())))
            .unwrap();
        app.receive();
        assert!(app.next_refresh <= Instant::now());
        assert_eq!(app.error, None, "the error belongs to open ones");
    }

    #[test]
    fn a_failed_merged_load_keeps_the_open_list() {
        let mut app = app();
        press(&mut app, KeyCode::Char('z'));
        app.loading = true;
        app.tx
            .send((Status::Merged, Err("gh: no network".into())))
            .unwrap();
        app.receive();
        assert_eq!(app.error.as_deref(), Some("gh: no network"));
        assert!(
            app.next_refresh > Instant::now(),
            "a failing forge is not asked again at once"
        );
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(iids(&app.rows(Section::Mine)), [6, 5, 4]);
        assert_eq!(app.error, None, "the open list has no error");
    }

    #[test]
    fn merged_ones_follow_the_scope() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        let project = app.project.clone();
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.fetch_request(), (Status::Merged, project.clone()));
        answer(&mut app, Status::Merged, merged_data(project.clone()));
        assert_eq!(iids(&app.rows(Section::Review)), [12, 11]);
        // All projects: these were asked for this project only.
        app.next_refresh = Instant::now() + Duration::from_secs(60);
        press(&mut app, KeyCode::Char('a'));
        assert!(app.rows(Section::Review).is_empty());
        assert!(app.loading_section(Section::Mine));
        assert!(app.next_refresh <= Instant::now(), "load at once");
        // An answer for the old scope does not count either.
        answer(&mut app, Status::Merged, merged_data(project));
        assert!(app.rows(Section::Mine).is_empty());
        assert!(app.next_refresh <= Instant::now());
        answer(&mut app, Status::Merged, merged_data(None));
        assert_eq!(iids(&app.rows(Section::Mine)), [13]);
        // Open ones keep switching at once, as before.
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(iids(&app.rows(Section::Mine)), [6, 5, 4]);
    }

    #[test]
    fn merged_and_closed_ones_cannot_be_approved_merged_drafted_or_branched() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        app.scope = Scope::All;
        press(&mut app, KeyCode::Char('z'));
        answer(&mut app, Status::Merged, merged_data(None));
        for key in ['v', 'm', 'w', 'b'] {
            app.flash = None;
            press(&mut app, KeyCode::Char(key));
            assert_eq!(flash(&app), Some("!13 is merged"), "{key}");
            assert!(app.confirm.is_none(), "{key}");
            assert!(!app.opening_branch, "{key}");
        }
        let mut closed = merged_data(None);
        closed.mine[0].status = Status::Closed;
        press(&mut app, KeyCode::Char('z'));
        answer(&mut app, Status::Closed, closed);
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(flash(&app), Some("!13 is closed"));
        assert!(app.confirm.is_none());
    }

    #[test]
    fn merged_ones_can_still_be_looked_at() {
        let mut app = app();
        press(&mut app, KeyCode::Char('z'));
        answer(&mut app, Status::Merged, merged_data(None));
        press(&mut app, KeyCode::Enter);
        // Reaches the browser step (the sample has no link), not a refusal.
        assert_eq!(flash(&app), Some("Only web links can be opened"));
        press(&mut app, KeyCode::Char('p'));
        assert!(app.details);
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.panel_tab, PanelTab::Activity);
        // Search and sort as on open ones.
        press(&mut app, KeyCode::Char('2'));
        app.filter = "change 11".into();
        assert_eq!(iids(&app.rows(Section::Review)), [11]);
        app.filter.clear();
        app.sort_by(SortKey::Number);
        app.sort_by(SortKey::Number);
        assert_eq!(iids(&app.rows(Section::Review)), [11, 12]);
    }

    #[test]
    fn quick_filters_are_off_for_merged_and_closed_ones() {
        let mut app = app();
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.show, Show::NeedsMe);
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.show, Show::All, "reset when leaving open");
        answer(&mut app, Status::Merged, merged_data(None));
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.show, Show::All);
        assert_eq!(
            flash(&app),
            Some("Quick filters are for open merge requests")
        );
        assert_eq!(iids(&app.rows(Section::Review)), [12, 11]);
    }

    #[test]
    fn clearing_keeps_the_state() {
        let mut app = app();
        press(&mut app, KeyCode::Char('z'));
        app.filter = "x".into();
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.status, Status::Merged);
        app.filter = "x".into();
        press(&mut app, KeyCode::Esc);
        assert!(app.filter.is_empty());
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.status, Status::Merged);
    }

    #[test]
    fn switching_state_or_scope_forgets_the_chosen_thread() {
        let mut app = app_in_clone(std::path::Path::new("/nowhere"));
        app.thread = Some(2);
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.thread, None, "z");
        app.thread = Some(2);
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.thread, None, "a");
    }

    #[test]
    fn a_write_makes_the_kept_merged_and_closed_lists_old() {
        let mut app = app();
        press(&mut app, KeyCode::Char('z'));
        answer(&mut app, Status::Merged, merged_data(None));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('z'));
        // Merging one of the open ones moves it to the merged list.
        app.after_write();
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(iids(&app.rows(Section::Mine)), [13], "shown at once");
        assert!(app.next_refresh <= Instant::now(), "and loaded again");
    }

    #[test]
    fn a_late_answer_does_not_reload_a_fresh_list() {
        let mut app = app();
        app.updated_at = Some(Instant::now());
        // A refresh of open ones runs; z goes round to open ones again.
        app.loading = true;
        for _ in 0..3 {
            press(&mut app, KeyCode::Char('z'));
        }
        assert_eq!(app.status, Status::Open);
        app.loading = true;
        answer(&mut app, Status::Merged, merged_data(None));
        assert!(
            app.next_refresh > Instant::now(),
            "open ones were loaded a moment ago"
        );
    }

    #[test]
    fn coming_back_keeps_where_each_section_was() {
        let mut app = app();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('2'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('z'));
        answer(&mut app, Status::Merged, merged_data(None));
        assert_eq!(app.review_state.selected(), Some(0), "a new list");
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.review_state.selected(), Some(1));
        assert_eq!(app.mine_state.selected(), Some(2));
        assert_eq!(app.selected().map(|mr| mr.iid), Some(2));
    }

    #[test]
    fn clicking_the_title_while_searching_switches_the_state() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('x'));
        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let title: String = (0..20).map(|x| buf[(x, 0)].symbol()).collect();
        click(&mut app, title.find("Merge").unwrap() as u16, 0);
        assert_eq!(app.status, Status::Merged);
        assert_eq!(app.filter, "x", "nothing typed into the search");
    }

    #[test]
    fn clicking_the_title_steps_the_state() {
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let title: String = (0..20).map(|x| buf[(x, 0)].symbol()).collect();
        let x = title.find("Merge").expect("title") as u16;
        click(&mut app, x, 0);
        assert_eq!(app.status, Status::Merged);
    }

    /// Comments of a merge request: an open thread on a line, a plain
    /// comment, and a resolved thread.
    fn discussions() -> Vec<crate::model::Discussion> {
        let note = |author: &str, body: &str, at: i64| crate::model::Note {
            author: author.into(),
            body: body.into(),
            created: Some(at),
            system: false,
        };
        vec![
            crate::model::Discussion {
                id: "gid://gitlab/Discussion/aaa".into(),
                resolvable: true,
                resolved: false,
                place: Some("src/a.rs:7".into()),
                notes: vec![note("sam", "Why?", 30)],
            },
            crate::model::Discussion {
                id: String::new(),
                resolvable: false,
                resolved: false,
                place: None,
                notes: vec![note("alex", "Nice.", 20)],
            },
            crate::model::Discussion {
                id: "gid://gitlab/Discussion/bbb".into(),
                resolvable: true,
                resolved: true,
                place: None,
                notes: vec![note("sam", "Typo.", 10)],
            },
        ]
    }

    /// The list with the details panel on the activity tab of !6, its
    /// comments loaded, and a poster that only says what it was given.
    fn in_activity() -> App {
        let mut app = app();
        app.details = true;
        app.panel_tab = PanelTab::Activity;
        let mr = app.selected().cloned().unwrap();
        app.activity
            .insert(mr.key(), (mr.updated, Activity::Loaded(discussions())));
        app.poster = echo;
        app
    }

    /// Never posts: fails with what it was asked to post, so tests see it.
    fn echo(mr: &MergeRequest, target: &Target, body: &str) -> Result<Posted, String> {
        let to = match target {
            Target::Reply { thread, .. } => format!("reply {thread}"),
            Target::Comment => "comment".to_string(),
            Target::RequestChanges => "changes".to_string(),
        };
        Err(format!("{} {} {to}: {body}", mr.project, mr.reference()))
    }

    fn posts(_: &MergeRequest, _: &Target, _: &str) -> Result<Posted, String> {
        Ok(Posted::Now)
    }

    fn pending(_: &MergeRequest, _: &Target, _: &str) -> Result<Posted, String> {
        Ok(Posted::InPendingReview)
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            match c {
                '\n' => press(app, KeyCode::Enter),
                c => press(app, KeyCode::Char(c)),
            }
        }
    }

    fn ctrl_s(app: &mut App) {
        app.key(KeyCode::Char('s'), KeyModifiers::CONTROL);
    }

    fn stage(app: &App) -> Option<Stage> {
        app.compose.as_ref().map(|c| c.stage)
    }

    fn text(app: &App) -> &str {
        app.compose
            .as_ref()
            .map(|c| c.field.text.as_str())
            .unwrap_or("")
    }

    /// Waits for the background post to answer.
    fn wait_posted(app: &mut App) {
        let until = Instant::now() + Duration::from_secs(10);
        while stage(app) == Some(Stage::Posting) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            app.receive_posted();
        }
        assert_ne!(stage(app), Some(Stage::Posting), "no answer");
    }

    /// Writes `body` and posts it with y.
    fn write_and_post(app: &mut App, key: char, body: &str) {
        press(app, KeyCode::Char(key));
        type_text(app, body);
        ctrl_s(app);
        press(app, KeyCode::Char('y'));
        wait_posted(app);
    }

    /// Comments of `key` loaded again, as the background load sends them.
    fn reload(app: &mut App, key: &ActivityKey, list: Vec<crate::model::Discussion>) {
        app.activity_seq += 1;
        let seq = app.activity_seq;
        app.activity_wanted.insert(key.clone(), seq);
        app.activity
            .insert(key.clone(), (Some(99), Activity::Loading));
        app.activity_tx.send((key.clone(), seq, Ok(list))).unwrap();
        app.receive_activity();
    }

    fn screen_text(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| ui::draw(frame, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn reply_opens_a_field_for_the_selected_thread() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.thread, Some(0));
        press(&mut app, KeyCode::Char('>'));
        let c = app.compose.as_ref().expect("a field");
        assert_eq!(
            c.target,
            Target::Reply {
                thread: "gid://gitlab/Discussion/aaa".into(),
                place: Some("src/a.rs:7".into()),
            }
        );
        assert_eq!(c.mr.iid, 6);
        assert!(screen_text(&mut app, 120, 40).contains("Reply to thread on src/a.rs:7 · !6"));
    }

    #[test]
    fn reply_without_a_thread_says_to_choose_one() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('>'));
        assert!(app.compose.is_none());
        assert_eq!(flash(&app), Some("Choose a thread first: n"));
        // Comments still loading: no thread to reply to either.
        let mut app = in_activity();
        app.activity.clear();
        app.thread = Some(0);
        press(&mut app, KeyCode::Char('>'));
        assert!(app.compose.is_none());
    }

    #[test]
    fn comment_opens_a_field_also_without_comments() {
        let mut app = in_activity();
        let key = app.selected().unwrap().key();
        app.activity
            .insert(key, (Some(6), Activity::Loaded(Vec::new())));
        assert!(screen_text(&mut app, 120, 40).contains("No activity yet."));
        press(&mut app, KeyCode::Char('+'));
        let c = app.compose.as_ref().expect("a field");
        assert_eq!(c.target, Target::Comment);
        assert!(screen_text(&mut app, 120, 40).contains("Comment on !6"));
    }

    #[test]
    fn reply_and_comment_keys_only_work_in_the_activity_tab() {
        let mut app = in_activity();
        app.thread = Some(0);
        app.panel_tab = PanelTab::Overview;
        press(&mut app, KeyCode::Char('>'));
        press(&mut app, KeyCode::Char('+'));
        assert!(app.compose.is_none());
        app.details = false;
        app.panel_tab = PanelTab::Activity;
        press(&mut app, KeyCode::Char('>'));
        press(&mut app, KeyCode::Char('+'));
        assert!(app.compose.is_none());
    }

    #[test]
    fn every_key_goes_into_the_field() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('+'));
        let section = app.section;
        type_text(&mut app, "q y n / r z a 1 2 c p ? d b v m x");
        app.key(KeyCode::Char('Q'), KeyModifiers::SHIFT);
        app.key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Down);
        assert_eq!(text(&app), "q y n / r z a 1 2 c p ? d b v m xQ    ");
        assert!(!app.ask_quit && !app.quit);
        assert!(!app.typing && app.filter.is_empty());
        assert!(!app.loading && !app.help && app.diff.is_none());
        assert_eq!(app.status, Status::Open);
        assert_eq!(app.scope, Scope::Current);
        assert_eq!(app.section, section);
        assert_eq!(app.panel_tab, PanelTab::Activity);
        assert!(app.details);
        assert_eq!(app.mine_state.selected(), Some(0));
        assert_eq!(stage(&app), Some(Stage::Writing));
    }

    #[test]
    fn ctrl_s_shows_what_will_be_posted_and_where() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('>'));
        type_text(&mut app, "Fixed in\nthe next commit");
        ctrl_s(&mut app);
        assert_eq!(stage(&app), Some(Stage::Preview));
        let shown = screen_text(&mut app, 150, 40);
        assert!(
            shown.contains("Post reply to thread on src/a.rs:7 · !6?"),
            "{shown}"
        );
        assert!(shown.contains("y post"), "{shown}");
        assert!(shown.contains("Fixed in"), "{shown}");
        assert!(shown.contains("the next commit"), "{shown}");
        // n goes back to the text.
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(stage(&app), Some(Stage::Writing));
        assert_eq!(text(&app), "Fixed in\nthe next commit");
    }

    #[test]
    fn only_y_posts_not_enter() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('+'));
        type_text(&mut app, "ok");
        ctrl_s(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(stage(&app), Some(Stage::Preview));
        assert_eq!(text(&app), "ok");
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(stage(&app), Some(Stage::Posting));
        wait_posted(&mut app);
        // The echo poster got it.
        let c = app.compose.as_ref().unwrap();
        assert_eq!(c.message.as_deref(), Some("acme/app !6 comment: ok"));
    }

    #[test]
    fn a_click_on_y_posts_and_other_clicks_do_nothing() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('>'));
        type_text(&mut app, "ok");
        // While writing: rows, tabs, the resolve button and key hints are dead.
        let mut terminal = Terminal::new(TestBackend::new(150, 40)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let spots: Vec<(Rect, Hit)> = app.hits.clone();
        for (r, hit) in &spots {
            if !matches!(hit, Hit::Compose(_)) {
                click(&mut app, r.x, r.y);
            }
        }
        assert_eq!(text(&app), "ok");
        assert_eq!(stage(&app), Some(Stage::Writing));
        assert_eq!(app.thread, Some(0));
        let found = discussions();
        let Some((_, Activity::Loaded(list))) =
            app.activity.get(&app.compose.as_ref().unwrap().mr.key())
        else {
            panic!("comments gone");
        };
        assert_eq!(list, &found, "nothing resolved");
        // Ctrl+s as a button, then y.
        let button = |app: &App, k: char| {
            app.hits
                .iter()
                .find(|(_, h)| *h == Hit::Compose(k))
                .map(|(r, _)| *r)
                .expect("button")
        };
        let send = button(&app, 's');
        click(&mut app, send.x, send.y);
        assert_eq!(stage(&app), Some(Stage::Preview));
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let yes = button(&app, 'y');
        click(&mut app, yes.x, yes.y);
        assert_eq!(stage(&app), Some(Stage::Posting));
    }

    #[test]
    fn esc_discards_while_writing_and_in_the_preview() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('+'));
        type_text(&mut app, "draft");
        press(&mut app, KeyCode::Esc);
        assert!(app.compose.is_none());
        assert_eq!(flash(&app), Some("Discarded the comment"));
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(text(&app), "", "a new field starts empty");
        type_text(&mut app, "draft");
        ctrl_s(&mut app);
        press(&mut app, KeyCode::Esc);
        assert!(app.compose.is_none());
        assert!(app.post_rx.try_recv().is_err(), "nothing was sent");
        // The list works as before.
        press(&mut app, KeyCode::Char('q'));
        assert!(app.ask_quit);
    }

    #[test]
    fn a_blank_text_is_not_posted() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('+'));
        ctrl_s(&mut app);
        assert_eq!(stage(&app), Some(Stage::Writing));
        type_text(&mut app, "  \n  ");
        ctrl_s(&mut app);
        assert_eq!(stage(&app), Some(Stage::Writing));
        assert_eq!(
            app.compose.as_ref().unwrap().message.as_deref(),
            Some("Nothing to post: write something first")
        );
        assert!(screen_text(&mut app, 120, 40).contains("Nothing to post"));
        // Typing again clears the note.
        type_text(&mut app, "x");
        assert_eq!(app.compose.as_ref().unwrap().message, None);
    }

    #[test]
    fn pasted_line_breaks_and_y_stay_in_the_text() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('+'));
        app.event(Event::Paste("first\ny\r\ny\n".into()));
        assert_eq!(text(&app), "first\ny\ny\n");
        assert_eq!(stage(&app), Some(Stage::Writing));
        // In the preview a paste changes nothing.
        type_text(&mut app, "end");
        ctrl_s(&mut app);
        app.event(Event::Paste("y".into()));
        assert_eq!(stage(&app), Some(Stage::Preview));
        assert_eq!(text(&app), "first\ny\ny\nend");
    }

    #[test]
    fn a_paste_goes_into_the_search_on_one_line() {
        let mut app = app();
        press(&mut app, KeyCode::Char('/'));
        app.event(Event::Paste("change\r\n5".into()));
        assert_eq!(app.filter, "change 5");
        assert_eq!(iids(&app.rows(Section::Mine)), [5]);
        // Outside of any field a paste does nothing (no y copies, no q).
        press(&mut app, KeyCode::Enter);
        app.event(Event::Paste("qy".into()));
        assert!(!app.ask_quit);
        assert_eq!(app.filter, "change 5");
    }

    #[test]
    fn a_reply_shows_under_its_thread_after_posting() {
        let mut app = in_activity();
        app.poster = posts;
        press(&mut app, KeyCode::Char('n'));
        write_and_post(&mut app, '>', "Done.");
        assert!(app.compose.is_none());
        assert_eq!(flash(&app), Some("Reply posted"));
        let key = app.selected().unwrap().key();
        assert!(!app.activity.contains_key(&key), "loads again without r");
        assert!(app.next_refresh <= Instant::now(), "and the list refreshes");
        // The reload has the reply; the thread moved to the top... of a list
        // where another thread came first.
        let mut list = discussions();
        list[0].notes.push(crate::model::Note {
            author: "me".into(),
            body: "Done.".into(),
            created: Some(40),
            system: false,
        });
        list.swap(0, 2);
        reload(&mut app, &key, list);
        assert_eq!(app.thread, Some(2), "the same thread, by its id");
        assert!(app.scroll_to_thread);
        assert!(screen_text(&mut app, 150, 40).contains("Done."));
    }

    #[test]
    fn a_comment_shows_at_the_top_after_posting() {
        let mut app = in_activity();
        app.poster = posts;
        app.details_scroll = 7;
        write_and_post(&mut app, '+', "Looks good");
        assert_eq!(flash(&app), Some("Comment posted"));
        assert_eq!(app.details_scroll, 0);
        let key = app.selected().unwrap().key();
        let mut list = discussions();
        list.insert(
            0,
            crate::model::Discussion {
                id: String::new(),
                resolvable: false,
                resolved: false,
                place: None,
                notes: vec![crate::model::Note {
                    author: "me".into(),
                    body: "Looks good".into(),
                    created: Some(50),
                    system: false,
                }],
            },
        );
        reload(&mut app, &key, list);
        assert!(screen_text(&mut app, 150, 40).contains("Looks good"));
    }

    #[test]
    fn a_reply_kept_in_a_pending_review_says_so() {
        let mut app = in_activity();
        app.poster = pending;
        press(&mut app, KeyCode::Char('n'));
        write_and_post(&mut app, '>', "Done.");
        assert_eq!(
            flash(&app),
            Some("Reply added to your pending review: submit it on GitHub")
        );
    }

    #[test]
    fn an_older_load_of_comments_does_not_hide_the_post() {
        let mut app = in_activity();
        app.poster = posts;
        let key = app.selected().unwrap().key();
        // A load runs while posting.
        app.activity_seq += 1;
        let old = app.activity_seq;
        app.activity_wanted.insert(key.clone(), old);
        write_and_post(&mut app, '+', "Looks good");
        reload(&mut app, &key, vec![]);
        // The old load answers last, without the comment.
        app.activity_tx
            .send((key.clone(), old, Ok(discussions())))
            .unwrap();
        app.receive_activity();
        let Some((_, Activity::Loaded(list))) = app.activity.get(&key) else {
            panic!("not loaded");
        };
        assert!(list.is_empty(), "the newer load stays");
    }

    #[test]
    fn a_reply_to_a_resolved_thread_leaves_it_resolved() {
        let mut app = in_activity();
        app.poster = posts;
        app.thread_filter = ThreadFilter::Resolved;
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.thread, Some(2));
        write_and_post(&mut app, '>', "Still fine.");
        assert_eq!(flash(&app), Some("Reply posted"));
        assert!(app.resolve_rx.try_recv().is_err(), "no resolve or reopen");
        let key = app.selected().unwrap().key();
        assert!(!app.activity.contains_key(&key), "loads again");
        reload(&mut app, &key, discussions());
        let Some((_, Activity::Loaded(list))) = app.activity.get(&key) else {
            panic!("not loaded");
        };
        assert!(list[2].resolved);
        assert_eq!(app.thread, Some(2));
    }

    #[test]
    fn a_failed_post_keeps_the_text_and_says_why() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('n'));
        write_and_post(&mut app, '>', "@alex see\nline two");
        assert_eq!(stage(&app), Some(Stage::Writing), "the field opens again");
        assert_eq!(text(&app), "@alex see\nline two");
        let why = "acme/app !6 reply gid://gitlab/Discussion/aaa: @alex see\nline two";
        assert_eq!(app.compose.as_ref().unwrap().message.as_deref(), Some(why));
        assert_eq!(flash(&app), Some(why));
        assert!(app.activity.contains_key(&app.selected().unwrap().key()));
        // It can be sent again from here.
        ctrl_s(&mut app);
        assert_eq!(stage(&app), Some(Stage::Preview));
        let shown = screen_text(&mut app, 150, 40);
        assert!(shown.contains("@alex see"), "{shown}");
    }

    #[test]
    fn a_refresh_while_writing_keeps_where_the_text_goes() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('>'));
        type_text(&mut app, "ok");
        // A refresh brings a newer merge request to the top of "Mine".
        let mut data = app.data.clone().unwrap();
        let mut newer = sample_mr(9);
        newer.updated = Some(100);
        data.mine.push(newer);
        data.mine.retain(|m| m.iid != 6);
        answer(&mut app, Status::Open, data);
        ctrl_s(&mut app);
        press(&mut app, KeyCode::Char('y'));
        wait_posted(&mut app);
        assert_eq!(
            app.compose.as_ref().unwrap().message.as_deref(),
            Some("acme/app !6 reply gid://gitlab/Discussion/aaa: ok")
        );
        // The box stays on screen although !6 left the list.
        assert!(screen_text(&mut app, 150, 40).contains("Reply to thread on src/a.rs:7 · !6"));
    }

    #[test]
    fn a_refresh_keeps_the_merge_request_selected_when_it_moves() {
        let mut app = app();
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected().map(|mr| mr.iid), Some(5));
        app.thread = Some(1);
        // A comment on !5 makes it the newest.
        let mut data = app.data.clone().unwrap();
        data.mine.iter_mut().find(|m| m.iid == 5).unwrap().updated = Some(100);
        answer(&mut app, Status::Open, data);
        assert_eq!(app.selected().map(|mr| mr.iid), Some(5));
        assert_eq!(app.mine_state.selected(), Some(0));
        assert_eq!(app.thread, Some(1), "same merge request, same thread");
        // Gone from the list: the next one, and no thread.
        let mut data = app.data.clone().unwrap();
        data.mine.retain(|m| m.iid != 5);
        answer(&mut app, Status::Open, data);
        assert_eq!(app.selected().map(|mr| mr.iid), Some(6));
        assert_eq!(app.thread, None);
    }

    #[test]
    fn a_github_pull_request_gets_replies_and_comments_too() {
        let mut app = in_activity();
        for mr in &mut app.data.as_mut().unwrap().mine {
            mr.forge = Forge::GitHub;
            mr.host = "github.com".into();
        }
        let mr = app.selected().cloned().unwrap();
        let mut list = discussions();
        list[0].id = "PRRT_kwDOabc".into();
        app.activity
            .insert(mr.key(), (mr.updated, Activity::Loaded(list)));
        press(&mut app, KeyCode::Char('n'));
        write_and_post(&mut app, '>', "ok");
        assert_eq!(
            app.compose.as_ref().unwrap().message.as_deref(),
            Some("acme/app #6 reply PRRT_kwDOabc: ok")
        );
        press(&mut app, KeyCode::Esc);
        write_and_post(&mut app, '+', "thanks");
        assert_eq!(
            app.compose.as_ref().unwrap().message.as_deref(),
            Some("acme/app #6 comment: thanks")
        );
    }

    /// The list on "To review" (someone else's !3 selected), with a
    /// poster that only says what it was given.
    fn reviewing() -> App {
        let mut app = app();
        app.section = Section::Review;
        app.clamp_selection();
        app.poster = echo;
        app
    }

    #[test]
    fn bang_opens_a_field_for_the_reason_in_the_list_and_the_panel() {
        let mut app = reviewing();
        assert_eq!(app.selected().unwrap().iid, 3);
        press(&mut app, KeyCode::Char('!'));
        let c = app.compose.as_ref().expect("a field");
        assert_eq!(c.target, Target::RequestChanges);
        assert_eq!(c.mr.iid, 3);
        assert!(screen_text(&mut app, 120, 40).contains("Request changes on !3"));
        // Also with the panel open, on either tab.
        for tab in [PanelTab::Overview, PanelTab::Activity] {
            let mut app = reviewing();
            app.details = true;
            app.panel_tab = tab;
            press(&mut app, KeyCode::Char('!'));
            assert!(app.compose.is_some(), "{tab:?}");
        }
    }

    #[test]
    fn own_ones_cannot_get_a_change_request() {
        let mut app = reviewing();
        app.section = Section::All;
        app.clamp_selection();
        app.move_by(1);
        let own = app.selected().cloned().unwrap();
        assert_eq!(own.author, own.viewer);
        press(&mut app, KeyCode::Char('!'));
        assert!(app.compose.is_none());
        assert_eq!(
            flash(&app),
            Some("You cannot request changes on your own merge request")
        );
    }

    #[test]
    fn merged_and_closed_ones_cannot_get_a_change_request() {
        let mut app = reviewing();
        for mr in &mut app.data.as_mut().unwrap().review {
            mr.status = Status::Merged;
        }
        press(&mut app, KeyCode::Char('!'));
        assert!(app.compose.is_none());
        assert_eq!(flash(&app), Some("!3 is merged"));
    }

    #[test]
    fn every_key_goes_into_the_reason_and_esc_sends_nothing() {
        let mut app = reviewing();
        press(&mut app, KeyCode::Char('!'));
        type_text(&mut app, "yqv!3 m\nsecond");
        press(&mut app, KeyCode::Backspace);
        assert_eq!(text(&app), "yqv!3 m\nsecon");
        assert!(app.confirm.is_none());
        assert_eq!(app.section, Section::Review);
        press(&mut app, KeyCode::Esc);
        assert!(app.compose.is_none());
        assert_eq!(flash(&app), Some("Discarded the change request"));
    }

    #[test]
    fn a_blank_reason_is_not_sent() {
        let mut app = reviewing();
        press(&mut app, KeyCode::Char('!'));
        type_text(&mut app, "  \n ");
        ctrl_s(&mut app);
        assert_eq!(stage(&app), Some(Stage::Writing));
        assert_eq!(
            app.compose.as_ref().unwrap().message.as_deref(),
            Some("Nothing to send: write what should change first")
        );
    }

    #[test]
    fn the_preview_names_the_merge_request_and_only_y_sends() {
        let mut app = reviewing();
        press(&mut app, KeyCode::Char('!'));
        type_text(&mut app, "Please split this.\nThanks");
        ctrl_s(&mut app);
        assert_eq!(stage(&app), Some(Stage::Preview));
        let shown = screen_text(&mut app, 150, 40);
        assert!(shown.contains("Request changes on !3 Change 3?"), "{shown}");
        assert!(shown.contains("y send"), "{shown}");
        assert!(shown.contains("Please split this."), "{shown}");
        assert!(shown.contains("Thanks"), "{shown}");
        // Enter does not send; n goes back to the text.
        press(&mut app, KeyCode::Enter);
        assert_eq!(stage(&app), Some(Stage::Preview));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(stage(&app), Some(Stage::Writing));
        ctrl_s(&mut app);
        press(&mut app, KeyCode::Esc);
        assert!(app.compose.is_none(), "Esc in the preview discards");
        // y sends the text to the merge request as a change request.
        write_and_post(&mut app, '!', "Please split this.");
        assert_eq!(
            app.compose.as_ref().unwrap().message.as_deref(),
            Some("acme/app !3 changes: Please split this.")
        );
    }

    #[test]
    fn a_sent_change_request_says_so_and_refreshes() {
        let mut app = reviewing();
        app.poster = posts;
        write_and_post(&mut app, '!', "Rename it.");
        assert!(app.compose.is_none());
        assert_eq!(flash(&app), Some("Changes requested on !3"));
        assert!(app.next_refresh <= Instant::now(), "the list refreshes");
        assert!(!app.activity.contains_key(&app.selected().unwrap().key()));
    }

    #[test]
    fn a_failed_change_request_keeps_the_reason_and_says_why() {
        let mut app = reviewing();
        write_and_post(&mut app, '!', "\"Grösse\" 👍 /merge later");
        assert_eq!(stage(&app), Some(Stage::Writing));
        assert_eq!(text(&app), "\"Grösse\" 👍 /merge later");
        let why = "acme/app !3 changes: \"Grösse\" 👍 /merge later";
        assert_eq!(app.compose.as_ref().unwrap().message.as_deref(), Some(why));
        // Out of time: it may have gone through.
        app.poster = times_out;
        ctrl_s(&mut app);
        press(&mut app, KeyCode::Char('y'));
        wait_posted(&mut app);
        let why = app.compose.as_ref().unwrap().message.clone().unwrap();
        assert!(
            why.contains("the change request may have been sent"),
            "{why}"
        );
    }

    #[test]
    fn one_i_approved_can_still_get_a_change_request() {
        let mut app = reviewing();
        let data = app.data.as_mut().unwrap();
        let mr = data.review.iter_mut().find(|m| m.iid == 3).unwrap();
        mr.approved_by = vec!["me".into()];
        mr.reviewers[0].state = ReviewState::Approved;
        press(&mut app, KeyCode::Char('!'));
        assert_eq!(
            app.compose.as_ref().map(|c| &c.target),
            Some(&Target::RequestChanges)
        );
    }

    #[test]
    fn a_long_reason_wraps_in_the_box_and_the_footer_stays_one_line() {
        let mut app = reviewing();
        press(&mut app, KeyCode::Char('!'));
        let long = "word ".repeat(120);
        type_text(&mut app, &long);
        ctrl_s(&mut app);
        for width in [150, 60] {
            let shown = screen_text(&mut app, width, 40);
            let last = shown.lines().last().unwrap();
            assert!(last.contains("y send"), "{width}: {last}");
            assert!(last.contains("Request changes"), "{width}: {last}");
        }
    }

    #[test]
    fn a_github_pull_request_gets_a_change_request_too() {
        let mut app = reviewing();
        for mr in &mut app.data.as_mut().unwrap().review {
            mr.forge = Forge::GitHub;
            mr.host = "github.com".into();
        }
        write_and_post(&mut app, '!', "ok");
        assert_eq!(
            app.compose.as_ref().unwrap().message.as_deref(),
            Some("acme/app #3 changes: ok")
        );
        press(&mut app, KeyCode::Esc);
        let mut app = reviewing();
        app.section = Section::All;
        app.clamp_selection();
        app.move_by(1);
        app.data.as_mut().unwrap().all.as_mut().unwrap().items[1].forge = Forge::GitHub;
        press(&mut app, KeyCode::Char('!'));
        assert_eq!(
            flash(&app),
            Some("You cannot request changes on your own pull request")
        );
    }

    #[test]
    fn help_names_the_change_request_key() {
        for (w, h) in [(150, 60), (150, 30), (70, 60)] {
            let mut app = app();
            app.help = true;
            let shown = screen_text(&mut app, w, h);
            assert!(
                shown.contains("request changes, with a reason (then y)"),
                "{w}x{h}\n{shown}"
            );
            assert!(
                shown.contains("b v ! m w: open ones only"),
                "{w}x{h}\n{shown}"
            );
        }
    }

    #[test]
    fn merged_and_closed_ones_take_no_replies_or_comments() {
        let mut app = app();
        press(&mut app, KeyCode::Char('z'));
        answer(&mut app, Status::Merged, merged_data(None));
        app.details = true;
        app.panel_tab = PanelTab::Activity;
        let mr = app.selected().cloned().unwrap();
        app.activity
            .insert(mr.key(), (mr.updated, Activity::Loaded(discussions())));
        press(&mut app, KeyCode::Char('n'));
        for key in ['>', '+'] {
            app.flash = None;
            press(&mut app, KeyCode::Char(key));
            assert!(app.compose.is_none(), "{key}");
            assert_eq!(flash(&app), Some("!13 is merged"), "{key}");
        }
        assert!(!screen_text(&mut app, 150, 40).contains("> reply"));
    }

    #[test]
    fn the_activity_tab_names_reply_and_comment() {
        let mut app = in_activity();
        let shown = screen_text(&mut app, 150, 40);
        assert!(shown.contains("> reply"), "{shown}");
        assert!(shown.contains("+ comment"), "{shown}");
        assert!(
            shown.contains("e resolve / reopen"),
            "each hint whole\n{shown}"
        );
        // A narrow panel wraps the hints instead of cutting them.
        let shown = screen_text(&mut app, 60, 40);
        assert!(shown.contains("+ comment"), "{shown}");
    }

    #[test]
    fn help_names_reply_comment_and_the_fields_keys() {
        for (w, h) in [(150, 60), (150, 30), (70, 60)] {
            let mut app = app();
            app.help = true;
            let shown = screen_text(&mut app, w, h);
            for k in [
                "open ones: all, needs me, threads, failed, ready",
                "close panel, search or filter (never the list)",
                "reply to the selected thread",
                "write a comment",
                "in the text: new line / send (then y) / discard",
            ] {
                assert!(shown.contains(k), "{w}x{h}: {k}\n{shown}");
            }
        }
        // Short but wide: two columns, so nothing is cut off.
        let mut app = app();
        app.help = true;
        let shown = screen_text(&mut app, 150, 30);
        assert!(shown.contains("Any key or click closes this."), "{shown}");
        assert!(shown.contains("double-click"), "{shown}");
    }

    fn times_out(_: &MergeRequest, _: &Target, _: &str) -> Result<Posted, String> {
        Err("gitlab.com did not answer within 30 s".into())
    }

    #[test]
    fn a_long_text_can_be_read_whole_in_the_preview() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('+'));
        let text: Vec<String> = (1..=30).map(|i| format!("row {i:02}")).collect();
        app.event(Event::Paste(text.join("\n")));
        let shown = screen_text(&mut app, 150, 40);
        assert!(
            shown.contains("row 30") && !shown.contains("row 01"),
            "the end while writing"
        );
        ctrl_s(&mut app);
        let shown = screen_text(&mut app, 150, 40);
        assert!(
            shown.contains("row 01") && !shown.contains("row 30"),
            "the start in the preview"
        );
        assert!(shown.contains("↑↓ scroll"), "{shown}");
        for _ in 0..40 {
            press(&mut app, KeyCode::Down);
        }
        let shown = screen_text(&mut app, 150, 40);
        assert!(shown.contains("row 30"), "{shown}");
        // The wheel over the box scrolls it too.
        let (area, _) = *app
            .hits
            .iter()
            .find(|(_, h)| *h == Hit::ComposeBox)
            .unwrap();
        for _ in 0..40 {
            app.mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: area.x + 2,
                row: area.y + 1,
                modifiers: KeyModifiers::NONE,
            });
        }
        let shown = screen_text(&mut app, 150, 40);
        assert!(shown.contains("row 01"), "{shown}");
        assert_eq!(stage(&app), Some(Stage::Preview), "scrolling posts nothing");
    }

    #[test]
    fn a_post_out_of_time_may_have_gone_through() {
        let mut app = in_activity();
        app.poster = times_out;
        write_and_post(&mut app, '+', "ok");
        let why = app.compose.as_ref().unwrap().message.clone().unwrap();
        assert_eq!(
            why,
            "gitlab.com did not answer within 30 s: the comment may have been posted, check above before sending again"
        );
        let key = app.selected().unwrap().key();
        assert!(!app.activity.contains_key(&key), "its comments load again");
        assert_eq!(text(&app), "ok");
    }

    #[test]
    fn what_other_writes_say_still_shows_while_writing() {
        let mut app = in_activity();
        press(&mut app, KeyCode::Char('+'));
        app.act_tx
            .send(Err("GitLab: pipeline must succeed".into()))
            .unwrap();
        app.receive_actions();
        let shown = screen_text(&mut app, 150, 40);
        let footer = shown.lines().last().unwrap();
        assert!(footer.contains("Ctrl+s send"), "{footer}");
        assert!(footer.contains("GitLab: pipeline must succeed"), "{footer}");
    }

    #[test]
    fn a_kept_thread_the_filter_hides_is_not_selected() {
        let mut app = in_activity();
        app.thread_filter = ThreadFilter::Open;
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.thread, Some(0));
        let key = app.selected().unwrap().key();
        app.keep_thread = Some((key.clone(), "gid://gitlab/Discussion/aaa".into()));
        // Someone resolved it meanwhile.
        let mut list = discussions();
        list[0].resolved = true;
        reload(&mut app, &key, list);
        assert_eq!(app.thread, None);
        press(&mut app, KeyCode::Char('>'));
        assert!(app.compose.is_none());
    }

    thread_local! {
        /// What the test opener was asked to open, per test thread.
        static OPENED: std::cell::RefCell<Vec<(String, String)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// An opener that starts nothing and only notes what it got.
    fn opens(opener: &str, url: &str) -> bool {
        OPENED.with(|o| o.borrow_mut().push((opener.into(), url.into())));
        true
    }

    fn cannot_open(opener: &str, url: &str) -> bool {
        opens(opener, url);
        false
    }

    fn opened() -> Vec<String> {
        OPENED.with(|o| o.borrow().iter().map(|(_, url)| url.clone()).collect())
    }

    /// The help open on a 100x42 pane, drawn once, and where its link to
    /// the repository is.
    fn help_with_link(opener: Opener) -> (App, Rect) {
        let mut app = app();
        app.opener = opener;
        app.help = true;
        screen_text(&mut app, 100, 42);
        let spot = app
            .hits
            .iter()
            .find_map(|(r, h)| matches!(h, Hit::Link(_)).then_some(*r))
            .expect("a link in the help");
        (app, spot)
    }

    const OPENER: &str = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };

    #[test]
    fn clicking_the_version_in_the_help_opens_the_repository() {
        let (mut app, spot) = help_with_link(opens);
        click(&mut app, spot.x + spot.width - 1, spot.y);
        assert_eq!(opened(), ["https://github.com/nolvane/herdr-git-panel"]);
        assert_eq!(OPENED.with(|o| o.borrow()[0].0.clone()), OPENER);
        assert!(!app.help, "the help closes");
        assert_eq!(flash(&app), Some("Opened in the browser"));
        // Its first cell counts too.
        let (mut app, spot) = help_with_link(opens);
        click(&mut app, spot.x, spot.y);
        assert_eq!(opened().len(), 2);
        assert!(!app.help);
    }

    #[test]
    fn a_failing_opener_is_named_and_the_list_goes_on() {
        let (mut app, spot) = help_with_link(cannot_open);
        click(&mut app, spot.x, spot.y);
        assert!(!app.help);
        assert_eq!(
            flash(&app),
            Some(format!("Could not run {OPENER}").as_str())
        );
        press(&mut app, KeyCode::Down);
        assert!(!app.quit);
    }

    #[test]
    fn other_clicks_and_keys_only_close_the_help() {
        let (mut app, spot) = help_with_link(opens);
        // On the border next to the version, inside the box, outside it.
        for (x, y) in [
            (spot.x - 1, spot.y),
            (spot.right(), spot.y),
            (50, 20),
            (0, 0),
        ] {
            app.help = true;
            screen_text(&mut app, 100, 42);
            click(&mut app, x, y);
            assert!(!app.help, "{x},{y}");
        }
        // A right click on the version only closes it too.
        app.help = true;
        screen_text(&mut app, 100, 42);
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: spot.x,
            row: spot.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(!app.help);
        for key in [KeyCode::Enter, KeyCode::Char('o'), KeyCode::Esc] {
            app.help = true;
            screen_text(&mut app, 100, 42);
            press(&mut app, key);
            assert!(!app.help, "{key:?}");
        }
        assert!(opened().is_empty(), "nothing opened: {:?}", opened());
        assert_eq!(flash(&app), None);
    }

    #[test]
    fn help_fits_a_middle_sized_pane() {
        let mut app = app();
        app.help = true;
        let shown = screen_text(&mut app, 100, 42);
        for k in [
            "Any key or click closes this.",
            "double-click",
            "reply to the selected thread",
            "ready",
        ] {
            assert!(shown.contains(k), "{k}\n{shown}");
        }
    }

    #[test]
    fn each_section_keeps_its_selection() {
        let mut app = app();
        assert_eq!(app.section, Section::Mine);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.mine_state.selected(), Some(2));
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.section, Section::Review);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.review_state.selected(), Some(1));
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.mine_state.selected(), Some(2));
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.review_state.selected(), Some(1));
    }
}
