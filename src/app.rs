//! The list's state and its event loop: keys, mouse, approvals, automatic
//! and manual refresh.

use crate::config::{Config, Scope};
use crate::forge::{self, Fetched};
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

/// The lists of a state that is not on screen, kept so switching back shows
/// them at once.
#[derive(Debug, Default)]
struct Parked {
    data: Option<Data>,
    error: Option<String>,
    updated_at: Option<Instant>,
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
            Some(p) => p.host == crate::github::HOST,
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
                Some(p) if section != Section::All => mr.project == p.path && mr.host == p.host,
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
        self.after_view_change();
        // Other merge requests now: a thread chosen before is not theirs.
        self.thread = None;
        // Missing, for the other scope or old: load at once (after the
        // refresh running now, if any). Otherwise as usual.
        let every = Duration::from_secs(self.config.refresh_secs);
        self.next_refresh = match self.updated_at {
            Some(at)
                if at.elapsed() < every
                    && !Section::ORDER.iter().any(|s| self.loading_section(*s)) =>
            {
                at + every
            }
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
    /// approved, merged, set to draft or opened as a branch.
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
        while let Ok((status, result)) = self.rx.try_recv() {
            self.loading = false;
            self.next_refresh = Instant::now() + Duration::from_secs(self.config.refresh_secs);
            if status != self.status {
                // Asked for before the state changed: keep it for that state,
                // and load the one on screen now.
                let parked = self.parked.entry(status).or_default();
                match result {
                    Ok(fetched) => {
                        parked.data = Some(fetched.data);
                        parked.error = fetched.warning;
                        parked.updated_at = Some(Instant::now());
                    }
                    Err(e) => parked.error = Some(e),
                }
                self.next_refresh = Instant::now();
                continue;
            }
            match result {
                Ok(fetched) => {
                    self.data = Some(fetched.data);
                    self.error = fetched.warning;
                    self.updated_at = Some(Instant::now());
                    self.clamp_selection();
                    // `a` was pressed while loading: "All" (or merged and
                    // closed lists) are for the other scope. Only after an
                    // answer, so a failing forge is not asked again at once,
                    // over and over.
                    if Section::ORDER.iter().any(|s| self.loading_section(*s)) {
                        self.next_refresh = Instant::now();
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
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
        for section in Section::ORDER {
            self.table_state_mut(section).select(Some(0));
        }
        self.thread = None;
        self.clamp_selection();
        // "All", and merged and closed ones, list other merge requests per
        // scope: load them now (the main loop starts it; one already running
        // is followed by another).
        if Section::ORDER.iter().any(|s| self.loading_section(*s)) {
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
        let mut app = app();
        let project = Project {
            host: "gitlab.com".into(),
            path: "acme/app".into(),
        };
        app.project = Some(project.clone());
        app.remotes = vec![crate::repo::Remote {
            name: "origin".into(),
            project,
        }];
        app.workdir = Some(dir.to_path_buf());
        app.herdr = true;
        app
    }

    fn flash(app: &App) -> Option<&str> {
        app.flash.as_ref().map(|(t, _)| t.as_str())
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
