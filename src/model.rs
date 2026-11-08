//! What the list shows, the same for GitLab merge requests and GitHub pull
//! requests.

/// Where a merge request lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Forge {
    GitLab,
    GitHub,
}

impl Forge {
    pub fn sign(self) -> char {
        match self {
            Forge::GitLab => '!',
            Forge::GitHub => '#',
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Forge::GitLab => "GitLab",
            Forge::GitHub => "GitHub",
        }
    }

    /// What the forge calls a change for review, in the singular.
    pub fn noun(self) -> &'static str {
        match self {
            Forge::GitLab => "merge request",
            Forge::GitHub => "pull request",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub title: String,
    /// `#rrggbb`.
    pub color: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Unreviewed,
    Started,
    Reviewed,
    ChangesRequested,
    Approved,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reviewer {
    pub username: String,
    pub state: ReviewState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pipeline {
    None,
    Running,
    Success,
    Failed,
    Canceled,
    Manual,
}

/// What stands between the merge request and a merge, in GitLab's words
/// (`detailedMergeStatus`), reduced to what the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeState {
    Ready,
    Draft,
    Conflict,
    NeedsRebase,
    Checking,
    Blocked,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeRequest {
    pub forge: Forge,
    /// The server, such as `gitlab.com` or `github.com`.
    pub host: String,
    /// Who is logged in on that server: "me" for this merge request.
    pub viewer: String,
    pub iid: u64,
    pub title: String,
    pub description: String,
    pub url: String,
    pub project: String,
    pub author: String,
    pub source_branch: String,
    /// The branch lives in another repository (a fork), so `source_branch`
    /// names a branch there, not in `project`.
    pub from_fork: bool,
    /// The commit at the head of the branch; a merge only goes through when
    /// it is still the same.
    pub head_sha: String,
    pub target_branch: String,
    pub created: Option<i64>,
    pub updated: Option<i64>,
    pub merge_state: MergeState,
    pub approvals_given: u64,
    /// Approvals needed in all, when the forge says (GitLab: the sum of its
    /// approval rules; GitHub: the branch protection, visible to admins).
    pub approvals_required: Option<u64>,
    pub approved_by: Vec<String>,
    pub approved: bool,
    /// Approval rules still waiting, in plain words ("Builders", "code owners").
    pub pending_rules: Vec<String>,
    pub reviewers: Vec<Reviewer>,
    pub threads_total: u64,
    pub threads_open: u64,
    pub labels: Vec<Label>,
    pub pipeline: Pipeline,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    /// How GitHub merges in this repository (`merge`, `squash`, `rebase`);
    /// empty on GitLab, which uses the project's own settings.
    pub merge_method: String,
}

impl MergeRequest {
    /// The number as the forge writes it: `!42` on GitLab, `#42` on GitHub.
    pub fn reference(&self) -> String {
        format!("{}{}", self.forge.sign(), self.iid)
    }

    /// Identifies the merge request across servers and projects.
    pub fn key(&self) -> (String, u64) {
        (format!("{}/{}", self.host, self.project), self.iid)
    }

    pub fn project_name(&self) -> &str {
        self.project.rsplit('/').next().unwrap_or(&self.project)
    }

    /// The review state of `user` on this merge request, if they review it.
    pub fn review_of(&self, user: &str) -> Option<ReviewState> {
        self.reviewers
            .iter()
            .find(|r| r.username == user)
            .map(|r| r.state)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Data {
    pub review: Vec<MergeRequest>,
    pub mine: Vec<MergeRequest>,
    /// The "All" section; `None` until it is loaded.
    pub all: Option<AllList>,
}

/// Every open merge request of one project, or of the projects you are a
/// member of, newest first, as far as one request per forge loads them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AllList {
    /// The project it was loaded for; `None` for your projects.
    pub of: Option<crate::repo::Project>,
    pub items: Vec<MergeRequest>,
    /// How many are open in all, as the forges count them; more than
    /// `items` when the list was cut.
    pub total: u64,
    /// A forge that failed to load it.
    pub error: Option<String>,
}

/// One forge's part of the "All" list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AllPart {
    pub items: Vec<MergeRequest>,
    pub total: u64,
    /// The projects it covers, as `host/path` (the first part of
    /// [`MergeRequest::key`]), so your own and the ones you review there
    /// can be added when they fall past the cut.
    pub projects: Vec<String>,
}

/// One note in a discussion.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub author: String,
    pub body: String,
    pub created: Option<i64>,
    /// Notes of the forge itself ("approved this merge request", "added 2 commits").
    pub system: bool,
}

/// A discussion: a thread of notes, maybe on a line of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Discussion {
    /// The forge's global id of the thread, to resolve it; empty for
    /// comments that cannot be resolved.
    pub id: String,
    pub resolvable: bool,
    pub resolved: bool,
    /// `path:line` when the thread is on a line of the changes.
    pub place: Option<String>,
    pub notes: Vec<Note>,
}
