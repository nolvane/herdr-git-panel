//! Sorting and filtering of the merge request lists.

use crate::model::{MergeRequest, MergeState, Pipeline, ReviewState};

/// What the list is sorted by. Clicking a column heading sorts by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Updated,
    Created,
    Number,
    Title,
    Project,
    Author,
    Threads,
    Size,
}

impl SortKey {
    /// The order `s` steps through.
    pub const CYCLE: [SortKey; 6] = [
        SortKey::Updated,
        SortKey::Created,
        SortKey::Threads,
        SortKey::Size,
        SortKey::Project,
        SortKey::Title,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SortKey::Updated => "updated",
            SortKey::Created => "opened",
            SortKey::Number => "number",
            SortKey::Title => "title",
            SortKey::Project => "project",
            SortKey::Author => "author",
            SortKey::Threads => "open threads",
            SortKey::Size => "size",
        }
    }

    /// Newest, biggest or most first; names A to Z.
    pub fn descending_by_default(self) -> bool {
        !matches!(self, SortKey::Title | SortKey::Project | SortKey::Author)
    }

    pub fn next(self) -> SortKey {
        let i = Self::CYCLE.iter().position(|k| *k == self).unwrap_or(0);
        Self::CYCLE[(i + 1) % Self::CYCLE.len()]
    }
}

/// Quick filters, stepped through with `f`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    All,
    /// To review: not approved by you yet. Mine: something to do (threads,
    /// failed pipeline, conflict, rebase).
    NeedsMe,
    OpenThreads,
    Failing,
    Ready,
}

impl Show {
    pub fn name(self) -> &'static str {
        match self {
            Show::All => "all",
            Show::NeedsMe => "needs me",
            Show::OpenThreads => "open threads",
            Show::Failing => "pipeline failed",
            Show::Ready => "ready to merge",
        }
    }

    pub fn next(self) -> Show {
        match self {
            Show::All => Show::NeedsMe,
            Show::NeedsMe => Show::OpenThreads,
            Show::OpenThreads => Show::Failing,
            Show::Failing => Show::Ready,
            Show::Ready => Show::All,
        }
    }

    pub fn keeps(self, mr: &MergeRequest, me: &str, reviewing: bool) -> bool {
        match self {
            Show::All => true,
            Show::NeedsMe if reviewing => !matches!(
                mr.review_of(me),
                Some(ReviewState::Approved) | Some(ReviewState::ChangesRequested)
            ),
            Show::NeedsMe => {
                mr.threads_open > 0
                    || mr.pipeline == Pipeline::Failed
                    || matches!(
                        mr.merge_state,
                        MergeState::Conflict | MergeState::NeedsRebase
                    )
            }
            Show::OpenThreads => mr.threads_open > 0,
            Show::Failing => mr.pipeline == Pipeline::Failed,
            Show::Ready => mr.merge_state == MergeState::Ready,
        }
    }
}

/// Whether the merge request matches the typed filter: every word must
/// appear in its number, title, project, author, branch or a label.
pub fn matches(mr: &MergeRequest, filter: &str) -> bool {
    let hay = format!(
        "{} {} {} {} {} {}",
        mr.reference(),
        mr.title,
        mr.project,
        mr.author,
        mr.source_branch,
        mr.labels
            .iter()
            .map(|l| l.title.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    )
    .to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|word| hay.contains(word))
}

pub fn sort(rows: &mut [&MergeRequest], key: SortKey, descending: bool) {
    rows.sort_by(|a, b| {
        let order = match key {
            SortKey::Updated => a.updated.cmp(&b.updated),
            SortKey::Created => a.created.cmp(&b.created),
            SortKey::Number => a.iid.cmp(&b.iid),
            SortKey::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            SortKey::Project => a.project.cmp(&b.project),
            SortKey::Author => a.author.cmp(&b.author),
            SortKey::Threads => a.threads_open.cmp(&b.threads_open),
            SortKey::Size => (a.additions.unwrap_or(0) + a.deletions.unwrap_or(0))
                .cmp(&(b.additions.unwrap_or(0) + b.deletions.unwrap_or(0))),
        };
        // Ties: newest first, so the order never jumps between refreshes.
        let order = if descending { order.reverse() } else { order };
        order.then(b.updated.cmp(&a.updated))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Label, MergeState, Pipeline};

    fn mr(iid: u64, title: &str, updated: i64, threads: u64) -> MergeRequest {
        MergeRequest {
            forge: crate::model::Forge::GitLab,
            host: "gitlab.com".into(),
            viewer: "me".into(),
            merge_method: String::new(),
            approvals_required: None,
            iid,
            title: title.into(),
            description: String::new(),
            url: String::new(),
            project: "acme/app".into(),
            author: "alex".into(),
            source_branch: "abc-1-thing".into(),
            head_sha: "abc123".into(),
            target_branch: "main".into(),
            created: Some(updated - 100),
            updated: Some(updated),
            merge_state: MergeState::Blocked,
            approvals_given: 0,
            approved_by: vec![],
            approved: false,
            pending_rules: vec![],
            reviewers: vec![],
            threads_total: threads,
            threads_open: threads,
            labels: vec![Label {
                title: "backend".into(),
                color: "#000000".into(),
            }],
            pipeline: Pipeline::Success,
            additions: Some(iid),
            deletions: Some(0),
        }
    }

    #[test]
    fn filter_words_all_match() {
        let m = mr(42, "Add export", 0, 0);
        assert!(matches(&m, "export"));
        assert!(matches(&m, "!42 BACKEND"));
        assert!(matches(&m, "abc-1"));
        assert!(!matches(&m, "export frontend"));
        assert!(matches(&m, ""));
    }

    #[test]
    fn sorting() {
        let (a, b, c) = (mr(1, "b", 30, 2), mr(2, "a", 10, 0), mr(3, "c", 20, 5));
        let mut rows = vec![&a, &b, &c];
        sort(&mut rows, SortKey::Updated, true);
        assert_eq!(rows.iter().map(|m| m.iid).collect::<Vec<_>>(), [1, 3, 2]);
        sort(&mut rows, SortKey::Title, false);
        assert_eq!(rows.iter().map(|m| m.iid).collect::<Vec<_>>(), [2, 1, 3]);
        sort(&mut rows, SortKey::Threads, true);
        assert_eq!(rows.iter().map(|m| m.iid).collect::<Vec<_>>(), [3, 1, 2]);
    }

    #[test]
    fn quick_filters() {
        let m = mr(1, "x", 0, 1);
        assert!(Show::OpenThreads.keeps(&m, "me", false));
        assert!(
            Show::NeedsMe.keeps(&m, "me", true),
            "not reviewed by me yet"
        );
        assert!(!Show::Ready.keeps(&m, "me", false));
        assert_eq!(Show::Ready.next(), Show::All);
    }
}
