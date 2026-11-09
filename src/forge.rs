//! One door to GitLab and GitHub: the list asks here, and each request goes
//! to the forge the merge request lives on.

use crate::cli;
use crate::model::*;
use crate::repo::Project;
use crate::{github, gitlab};

/// The lists of every forge whose tool is installed and logged in.
pub struct Fetched {
    pub data: Data,
    /// A forge that failed while another one answered.
    pub warning: Option<String>,
}

/// Which forges "All" asks: only the one `project` lives on, or every forge
/// for your projects (`None`). As `(GitLab, GitHub)`.
pub fn all_forges(project: Option<&Project>, gitlab_host: &str) -> (bool, bool) {
    match project {
        Some(p) => (p.on(gitlab_host), p.on(github::HOST)),
        None => (true, true),
    }
}

/// Whether `project` lives on a forge the list knows: GitHub or the
/// configured GitLab host.
pub fn known(project: &Project, gitlab_host: &str) -> bool {
    project.on(gitlab_host) || project.on(github::HOST)
}

/// Which forges "Mine" and "To review" ask, and for which project: open
/// ones always every forge for every project (the list filters them, so `a`
/// switches at once); merged and closed ones, of which only the newest
/// load, only the forge of `all_of` for that project, so they are its own
/// newest. As `(GitLab, GitHub, project path)`.
pub fn list_forges<'a>(
    status: Status,
    all_of: Option<&'a Project>,
    gitlab_host: &str,
) -> (bool, bool, Option<&'a str>) {
    match status {
        Status::Open => (true, true, None),
        _ => {
            let (lab, hub) = all_forges(all_of, gitlab_host);
            (lab, hub, all_of.map(|p| p.path.as_str()))
        }
    }
}

/// Whether a refresh asks GitLab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskGitLab {
    /// It answered with its lists before: every error, a 429 too, shows.
    Yes,
    /// Left out: it answered "not logged in" and its login has not changed.
    No,
    /// Not known to be logged in (the list just opened, it answered "not
    /// logged in" last time, or its login changed): a 429 then is most
    /// likely gitlab.com's limit for traffic without a login, which glab
    /// can't tell from another one, so it counts as "not logged in".
    Again,
}

/// What GitLab said about its login in a refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitLabLogin {
    /// Not asked: left out, glab missing, or not needed for the project.
    NotAsked,
    /// Every list asked answered "not logged in".
    LoggedOut,
    /// It answered with lists.
    LoggedIn,
    /// A real error, no lists.
    Failed,
}

/// The lists in `status` of every forge, and "All" for `all_of` (`None`:
/// your projects), with what GitLab said about its login.
pub fn fetch_all(
    gitlab_host: &str,
    status: Status,
    all_of: Option<&Project>,
    gitlab: AskGitLab,
) -> (GitLabLogin, Result<Fetched, String>) {
    let has_gitlab = cli::find("glab").is_some();
    let use_github = github::available();
    if !has_gitlab && !use_github {
        return (
            GitLabLogin::NotAsked,
            Err(
                "Neither glab nor gh is installed: brew install glab (GitLab) or gh (GitHub)"
                    .into(),
            ),
        );
    }
    let use_gitlab = has_gitlab && gitlab != AskGitLab::No;
    let (all_gitlab, all_github) = all_forges(all_of, gitlab_host);
    let (lists_gitlab, lists_github, lists_of) = list_forges(status, all_of, gitlab_host);
    let path = all_of.map(|p| p.path.as_str());
    let (lab, hub, lab_all, hub_all) = std::thread::scope(|scope| {
        let hub = scope.spawn(|| {
            if use_github && lists_github {
                github::fetch(status, lists_of)
            } else {
                Ok(None)
            }
        });
        let lab_all = scope.spawn(|| {
            if use_gitlab && all_gitlab {
                gitlab::fetch_all_of(gitlab_host, status, path)
            } else {
                Ok(None)
            }
        });
        let hub_all = scope.spawn(|| {
            if use_github && all_github {
                github::fetch_all_of(status, path)
            } else {
                Ok(None)
            }
        });
        let lab = if use_gitlab && lists_gitlab {
            gitlab::fetch(gitlab_host, status, lists_of)
        } else {
            Ok(None)
        };
        (
            lab,
            hub.join().unwrap_or_else(|_| Err("gh crashed".into())),
            lab_all
                .join()
                .unwrap_or_else(|_| Err("glab crashed".into())),
            hub_all.join().unwrap_or_else(|_| Err("gh crashed".into())),
        )
    });
    let asked = use_gitlab && (lists_gitlab || all_gitlab);
    let (login, lab, lab_all) = gitlab_login(asked, gitlab, lab, lab_all);
    (
        login,
        combine(
            gitlab_host,
            [has_gitlab, use_github],
            status,
            all_of,
            [lab, hub],
            [lab_all, hub_all],
        ),
    )
}

/// A forge's lists: `None` when it was not asked or is not logged in.
type Lists = Result<Option<Data>, String>;
/// A forge's part of "All", likewise.
type All = Result<Option<AllPart>, String>;

/// What GitLab's answers say about its login, and the answers with a 429
/// taken as "not logged in" when it was asked [`AskGitLab::Again`].
fn gitlab_login(
    asked: bool,
    gitlab: AskGitLab,
    lists: Lists,
    all: All,
) -> (GitLabLogin, Lists, All) {
    if !asked {
        return (GitLabLogin::NotAsked, lists, all);
    }
    fn quiet<T>(again: bool, r: Result<Option<T>, String>) -> Result<Option<T>, String> {
        match r {
            Err(e) if again && gitlab::limited(&e) => Ok(None),
            r => r,
        }
    }
    // Lists in one half prove the login: then a 429 in the other is an
    // error to show.
    let answered = matches!(lists, Ok(Some(_))) || matches!(all, Ok(Some(_)));
    let again = gitlab == AskGitLab::Again && !answered;
    let (lists, all) = (quiet(again, lists), quiet(again, all));
    let login = match (&lists, &all) {
        (Ok(None), Ok(None)) => GitLabLogin::LoggedOut,
        (Ok(Some(_)), _) | (_, Ok(Some(_))) => GitLabLogin::LoggedIn,
        _ => GitLabLogin::Failed,
    };
    (login, lists, all)
}

/// Puts the forges' answers together: `installed`, `lists` and `all` are
/// GitLab's and GitHub's, in that order. "All" failing never fails the
/// other lists.
fn combine(
    gitlab_host: &str,
    installed: [bool; 2],
    status: Status,
    all_of: Option<&Project>,
    lists: [Result<Option<Data>, String>; 2],
    all: [Result<Option<AllPart>, String>; 2],
) -> Result<Fetched, String> {
    let mut data = Data::default();
    let mut errors = Vec::new();
    let mut answered = [false; 2];
    for (i, result) in lists.into_iter().enumerate() {
        match result {
            Ok(Some(d)) => {
                answered[i] = true;
                data.review.extend(d.review);
                data.mine.extend(d.mine);
            }
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if !answered.contains(&true) {
        return Err(if !errors.is_empty() {
            errors.join(" · ")
        } else if let (Some(p), false) = (all_of, status == Status::Open) {
            // Only this project's forge was asked.
            not_available(p)
        } else {
            not_logged_in(gitlab_host, installed)
        });
    }
    // Merged and closed ones were asked for this project only.
    if status != Status::Open {
        data.of = all_of.cloned();
    }
    let mut list = AllList {
        of: all_of.cloned(),
        ..AllList::default()
    };
    let mut all_errors = Vec::new();
    let mut covered: Vec<String> = Vec::new();
    let mut answered_all = false;
    for (i, result) in all.into_iter().enumerate() {
        match result {
            Ok(Some(part)) => {
                answered_all = true;
                list.items.extend(part.items);
                list.total += part.total;
                covered.extend(part.projects);
            }
            Ok(None) => {}
            Err(e) => {
                // A forge whose lists failed too says so once already.
                if answered[i] {
                    errors.push(format!("All: {e}"));
                }
                all_errors.push(e);
            }
        }
    }
    // A project nobody could be asked about is not "no open merge
    // requests": say why.
    if let (Some(p), false, true) = (all_of, answered_all, all_errors.is_empty()) {
        all_errors.push(not_available(p));
    }
    list.error = (!all_errors.is_empty()).then(|| all_errors.join(" · "));
    list.newest = list.items.len();
    // Your own and the ones you review in those projects are in "All" too,
    // also when they are older than what one request loads.
    for mr in data.mine.iter().chain(&data.review) {
        let key = mr.key();
        if covered.contains(&key.0) && !list.items.iter().any(|m| m.key() == key) {
            list.items.push(mr.clone());
        }
    }
    list.total = list.total.max(list.items.len() as u64);
    for list in [&mut data.review, &mut data.mine, &mut list.items] {
        list.sort_by_key(|m| std::cmp::Reverse(m.updated));
    }
    data.all = Some(list);
    Ok(Fetched {
        data,
        warning: (!errors.is_empty()).then(|| errors.join(" · ")),
    })
}

/// Why no forge answered although a tool is installed: each installed one
/// is not logged in, and how to log in.
fn not_logged_in(gitlab_host: &str, installed: [bool; 2]) -> String {
    let mut hints = Vec::new();
    if installed[0] {
        hints.push(cli::login_hint("glab", gitlab_host));
    }
    if installed[1] {
        hints.push(cli::login_hint("gh", github::HOST));
    }
    hints.join(" · ")
}

/// Why nothing could be asked about `p`: its forge's tool is missing or not
/// logged in.
fn not_available(p: &Project) -> String {
    let tool = if p.on(github::HOST) { "gh" } else { "glab" };
    // The host as the tools write it, whatever the remote's spelling.
    let host = p.host.to_ascii_lowercase();
    format!("{tool} is not installed or not logged in to {host}")
}

pub fn fetch_activity(mr: &MergeRequest) -> Result<Vec<Discussion>, String> {
    match mr.forge {
        Forge::GitLab => gitlab::fetch_activity(mr),
        Forge::GitHub => github::fetch_activity(mr),
    }
}

pub fn set_resolved(mr: &MergeRequest, thread: &str, resolve: bool) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::set_resolved(&mr.host, thread, resolve),
        Forge::GitHub => github::set_resolved(thread, resolve),
    }
}

/// How a forge took a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posted {
    /// Everyone sees it now.
    Now,
    /// GitHub added it to a review of yours that is not submitted yet.
    InPendingReview,
}

/// Replies `body` to the thread with the forge's id `thread`.
pub fn reply(mr: &MergeRequest, thread: &str, body: &str) -> Result<Posted, String> {
    match mr.forge {
        Forge::GitLab => gitlab::post_note(mr, Some(thread), body).map(|()| Posted::Now),
        Forge::GitHub => github::reply(thread, body).map(|pending| {
            if pending {
                Posted::InPendingReview
            } else {
                Posted::Now
            }
        }),
    }
}

/// Posts `body` as a comment on the merge request itself.
pub fn comment(mr: &MergeRequest, body: &str) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::post_note(mr, None, body),
        Forge::GitHub => github::comment(mr, body),
    }
}

/// Requests changes on the merge request, with `reason` as the text.
pub fn request_changes(mr: &MergeRequest, reason: &str) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::request_changes(mr, reason),
        Forge::GitHub => github::request_changes(mr, reason),
    }
}

pub fn set_draft(mr: &MergeRequest, draft: bool) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::set_draft(mr, draft),
        Forge::GitHub => github::set_draft(mr, draft),
    }
}

pub fn approve(mr: &MergeRequest) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::set_approval(mr, false),
        Forge::GitHub => github::approve(mr),
    }
}

/// Whether your approval can be taken back from the list.
pub fn can_revoke(forge: Forge) -> bool {
    forge == Forge::GitLab
}

pub fn revoke(mr: &MergeRequest) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::set_approval(mr, true),
        Forge::GitHub => Err("GitHub cannot take an approval back".into()),
    }
}

pub fn merge(mr: &MergeRequest) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::merge(mr),
        Forge::GitHub => github::merge(mr),
    }
}

pub fn fetch_diff(mr: &MergeRequest) -> Result<String, String> {
    match mr.forge {
        Forge::GitLab => gitlab::fetch_diff(mr),
        Forge::GitHub => github::fetch_diff(mr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(host: &str) -> Project {
        Project {
            host: host.into(),
            path: "acme/app".into(),
        }
    }

    #[test]
    fn all_asks_only_the_projects_forge() {
        assert_eq!(
            all_forges(Some(&project("gitlab.com")), "gitlab.com"),
            (true, false)
        );
        assert_eq!(
            all_forges(Some(&project("github.com")), "gitlab.com"),
            (false, true)
        );
        assert_eq!(
            all_forges(Some(&project("git.acme.test")), "git.acme.test"),
            (true, false)
        );
        // Your projects: every forge.
        assert_eq!(all_forges(None, "gitlab.com"), (true, true));
    }

    #[test]
    fn a_host_in_other_case_is_the_same_forge() {
        // Hosts ignore case, as written in the clone's remote or the setting.
        assert_eq!(
            all_forges(Some(&project("GitHub.com")), "gitlab.com"),
            (false, true)
        );
        assert_eq!(
            all_forges(Some(&project("GitLab.com")), "gitlab.com"),
            (true, false)
        );
        assert_eq!(
            all_forges(Some(&project("git.acme.test")), "Git.Acme.test"),
            (true, false)
        );
        assert!(known(&project("GitHub.com"), "gitlab.com"));
        assert!(known(&project("GITLAB.COM"), "gitlab.com"));
        assert!(known(&project("git.acme.test"), "Git.Acme.Test"));
        assert!(!known(&project("work-github"), "gitlab.com"));
        assert!(!known(&project("github.com.evil.test"), "gitlab.com"));
        assert_eq!(
            not_available(&project("GitHub.com")),
            "gh is not installed or not logged in to github.com"
        );
    }

    fn mr(forge: Forge, iid: u64, updated: i64) -> MergeRequest {
        let mut d = crate::gitlab::parse(
            &serde_json::json!({"data": {"currentUser": {"username": "me", "authoredMergeRequests": {"nodes": [
                {"iid": iid.to_string(), "project": {"fullPath": "acme/app"}, "author": {"username": "alex"}}
            ]}}}}),
            "gitlab.com",
        )
        .unwrap();
        let mut m = d.mine.remove(0);
        m.forge = forge;
        m.updated = Some(updated);
        m
    }

    fn lists() -> Data {
        Data {
            review: vec![mr(Forge::GitLab, 1, 1)],
            mine: vec![mr(Forge::GitLab, 2, 2)],
            of: None,
            all: None,
        }
    }

    #[test]
    fn merged_and_closed_lists_ask_only_the_projects_forge() {
        let p = project("gitlab.com");
        // Open ones: every forge, every project, as always.
        assert_eq!(
            list_forges(Status::Open, Some(&p), "gitlab.com"),
            (true, true, None)
        );
        assert_eq!(
            list_forges(Status::Merged, Some(&p), "gitlab.com"),
            (true, false, Some("acme/app"))
        );
        let hub = project("github.com");
        assert_eq!(
            list_forges(Status::Closed, Some(&hub), "gitlab.com"),
            (false, true, Some("acme/app"))
        );
        assert_eq!(
            list_forges(Status::Merged, None, "gitlab.com"),
            (true, true, None)
        );
    }

    #[test]
    fn merged_lists_remember_their_project() {
        let p = project("gitlab.com");
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Merged,
            Some(&p),
            [Ok(Some(lists())), Ok(None)],
            [Ok(None), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.data.of, Some(p.clone()));
        // Open ones are for every project.
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            Some(&p),
            [Ok(Some(lists())), Ok(None)],
            [Ok(None), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.data.of, None);
    }

    #[test]
    fn merged_ones_of_a_project_nobody_could_be_asked_about_say_why() {
        // glab missing, in a GitLab project: GitHub is not asked either.
        let Err(e) = combine(
            "gitlab.com",
            BOTH,
            Status::Merged,
            Some(&project("gitlab.com")),
            [Ok(None), Ok(None)],
            [Ok(None), Ok(None)],
        ) else {
            panic!("an empty list instead of the reason");
        };
        assert_eq!(e, "glab is not installed or not logged in to gitlab.com");
        // Your projects: a forge without its tool is left out quietly.
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Closed,
            None,
            [Ok(None), Ok(Some(lists()))],
            [Ok(None), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.warning, None);
        assert_eq!(fetched.data.mine.len(), 1);
        // One forge failing keeps the other's, with the error in the footer.
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Closed,
            None,
            [Err("glab: no network".into()), Ok(Some(lists()))],
            [Ok(None), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.warning.as_deref(), Some("glab: no network"));
        assert_eq!(fetched.data.review.len(), 1);
    }

    #[test]
    fn all_failing_keeps_the_other_lists() {
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            Some(&project("gitlab.com")),
            [Ok(Some(lists())), Ok(None)],
            [Err("GitLab: boom".into()), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.data.review.len(), 1);
        assert_eq!(fetched.data.mine.len(), 1);
        assert_eq!(fetched.warning.as_deref(), Some("All: GitLab: boom"));
        let all = fetched.data.all.unwrap();
        assert!(all.items.is_empty());
        assert_eq!(all.error.as_deref(), Some("GitLab: boom"));
        assert_eq!(all.of, Some(project("gitlab.com")));
    }

    #[test]
    fn all_has_mine_and_the_ones_i_review_past_the_cut() {
        let mut other_project = mr(Forge::GitLab, 9, 9);
        other_project.project = "acme/other".into();
        let mut data = lists();
        data.mine.push(other_project);
        let part = AllPart {
            // The project's newest, which already holds my !2.
            items: vec![mr(Forge::GitLab, 3, 30), mr(Forge::GitLab, 2, 2)],
            total: 140,
            projects: vec!["gitlab.com/acme/app".into()],
        };
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            Some(&project("gitlab.com")),
            [Ok(Some(data)), Ok(None)],
            [Ok(Some(part)), Ok(None)],
        )
        .unwrap();
        let all = fetched.data.all.unwrap();
        let iids: Vec<u64> = all.items.iter().map(|m| m.iid).collect();
        assert_eq!(iids, [3, 2, 1], "each once, none of another project");
        assert_eq!(all.total, 140);
        assert_eq!(all.newest, 2, "the forge's newest, without mine");
    }

    #[test]
    fn a_project_nobody_was_asked_about_says_why() {
        // gh only, in a GitLab project.
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            Some(&project("gitlab.com")),
            [Ok(None), Ok(Some(lists()))],
            [Ok(None), Ok(None)],
        )
        .unwrap();
        let all = fetched.data.all.unwrap();
        assert_eq!(
            all.error.as_deref(),
            Some("glab is not installed or not logged in to gitlab.com")
        );
        // Your projects: a forge without its tool is simply left out.
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            None,
            [Ok(None), Ok(Some(lists()))],
            [Ok(None), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.data.all.unwrap().error, None);
    }

    #[test]
    fn a_forge_that_failed_says_it_once() {
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            None,
            [Err("glab: not logged in".into()), Ok(Some(lists()))],
            [Err("glab: not logged in".into()), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.warning.as_deref(), Some("glab: not logged in"));
    }

    /// glab and gh both installed.
    const BOTH: [bool; 2] = [true, true];

    #[test]
    fn gitlab_not_logged_in_is_left_out_quietly() {
        // glab not logged in (`Ok(None)` for its lists and "All"), gh
        // answering: GitHub's lists, nothing in the footer.
        for status in [Status::Open, Status::Merged] {
            let fetched = combine(
                "gitlab.com",
                BOTH,
                status,
                None,
                [Ok(None), Ok(Some(lists()))],
                [Ok(None), Ok(Some(AllPart::default()))],
            )
            .unwrap();
            assert_eq!(fetched.warning, None, "{status:?}");
            assert_eq!(fetched.data.mine.len(), 1);
            assert_eq!(fetched.data.all.unwrap().error, None);
        }
        // In a clone of a GitLab project, "All" says why it is empty.
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            Some(&project("gitlab.com")),
            [Ok(None), Ok(Some(lists()))],
            [Ok(None), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.warning, None);
        assert_eq!(
            fetched.data.all.unwrap().error.as_deref(),
            Some("glab is not installed or not logged in to gitlab.com")
        );
        // And the other way round: gh not logged in, GitLab answering.
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            None,
            [Ok(Some(lists())), Ok(None)],
            [Ok(Some(AllPart::default())), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.warning, None);
        assert_eq!(fetched.data.review.len(), 1);
    }

    #[test]
    fn nobody_logged_in_names_the_tools_that_are_installed() {
        let nobody = |installed: [bool; 2]| {
            combine(
                "git.acme.test",
                installed,
                Status::Open,
                None,
                [Ok(None), Ok(None)],
                [Ok(None), Ok(None)],
            )
            .err()
            .unwrap()
        };
        assert_eq!(
            nobody([true, false]),
            "glab is not logged in to git.acme.test: run glab auth login --hostname git.acme.test"
        );
        assert_eq!(
            nobody([false, true]),
            "gh is not logged in to github.com: run gh auth login"
        );
        assert_eq!(
            nobody(BOTH),
            "glab is not logged in to git.acme.test: run glab auth login --hostname git.acme.test · \
             gh is not logged in to github.com: run gh auth login"
        );
    }

    #[test]
    fn all_of_both_forges_is_one_list_newest_first() {
        let lab = AllPart {
            items: vec![mr(Forge::GitLab, 3, 30), mr(Forge::GitLab, 4, 10)],
            total: 140,
            projects: Vec::new(),
        };
        let hub = AllPart {
            items: vec![mr(Forge::GitHub, 5, 20)],
            total: 54,
            projects: Vec::new(),
        };
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            None,
            [Ok(Some(lists())), Ok(Some(Data::default()))],
            [Ok(Some(lab)), Ok(Some(hub))],
        )
        .unwrap();
        let all = fetched.data.all.unwrap();
        let iids: Vec<u64> = all.items.iter().map(|m| m.iid).collect();
        assert_eq!(iids, [3, 5, 4]);
        assert_eq!(all.total, 194);
        assert_eq!(all.of, None);
        assert_eq!(fetched.warning, None);
    }

    fn part() -> AllPart {
        AllPart {
            items: vec![mr(Forge::GitLab, 3, 3)],
            total: 1,
            projects: vec!["gitlab.com/acme/app".into()],
        }
    }

    const LIMIT: &str = "glab failed: glab: HTTP 429";

    #[test]
    fn gitlab_says_whether_it_is_logged_in() {
        use AskGitLab::*;
        let login = |ask, lists, all| gitlab_login(true, ask, lists, all).0;
        assert_eq!(login(Yes, Ok(None), Ok(None)), GitLabLogin::LoggedOut);
        assert_eq!(
            login(Yes, Ok(Some(lists())), Ok(Some(part()))),
            GitLabLogin::LoggedIn
        );
        // Lists with "All" failing: logged in, the error shows.
        assert_eq!(
            login(Yes, Ok(Some(lists())), Err("boom".into())),
            GitLabLogin::LoggedIn
        );
        assert_eq!(
            login(Yes, Err("boom".into()), Ok(None)),
            GitLabLogin::Failed
        );
        // Left out: nothing said.
        let (said, lists, all) = gitlab_login(false, No, Ok(None), Ok(None));
        assert_eq!(said, GitLabLogin::NotAsked);
        assert!(matches!((lists, all), (Ok(None), Ok(None))));
    }

    #[test]
    fn a_429_without_a_known_login_is_not_logged_in() {
        // Not known as logged in: gitlab.com's limit for traffic without a
        // login, quiet, also when only one of the requests got it.
        for (lists, all) in [
            (Err(LIMIT.to_string()), Err(LIMIT.to_string())),
            (Ok(None), Err(LIMIT.to_string())),
            (Err(LIMIT.to_string()), Ok(None)),
        ] {
            let (said, lists, all) = gitlab_login(true, AskGitLab::Again, lists, all);
            assert_eq!(said, GitLabLogin::LoggedOut);
            let fetched = combine(
                "gitlab.com",
                BOTH,
                Status::Open,
                None,
                [lists, Ok(Some(lists_of_github()))],
                [all, Ok(None)],
            )
            .unwrap();
            assert_eq!(fetched.warning, None, "nothing in the footer");
            assert_eq!(fetched.data.mine.len(), 1, "GitHub's lists stay");
        }
        // Lists prove the login: a 429 for "All" is an error to show.
        let (said, lists, all) =
            gitlab_login(true, AskGitLab::Again, Ok(Some(lists())), Err(LIMIT.into()));
        assert_eq!(said, GitLabLogin::LoggedIn);
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            Some(&project("gitlab.com")),
            [lists, Ok(None)],
            [all, Ok(None)],
        )
        .unwrap();
        assert_eq!(
            fetched.warning.as_deref(),
            Some("All: glab failed: glab: HTTP 429")
        );
        // Another error stays one.
        let (said, lists, _) = gitlab_login(
            true,
            AskGitLab::Again,
            Err("glab: no network".into()),
            Ok(None),
        );
        assert_eq!(said, GitLabLogin::Failed);
        assert!(lists.is_err());
    }

    #[test]
    fn a_429_while_logged_in_shows() {
        let (said, lists, all) =
            gitlab_login(true, AskGitLab::Yes, Err(LIMIT.into()), Err(LIMIT.into()));
        assert_eq!(said, GitLabLogin::Failed);
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            None,
            [lists, Ok(Some(lists_of_github()))],
            [all, Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.warning.as_deref(), Some(LIMIT));
    }

    fn lists_of_github() -> Data {
        Data {
            review: vec![],
            mine: vec![mr(Forge::GitHub, 5, 5)],
            of: None,
            all: None,
        }
    }

    #[test]
    fn gitlab_left_out_still_says_why() {
        // In a GitLab project's clone, "All" is not "no merge requests".
        let fetched = combine(
            "gitlab.com",
            BOTH,
            Status::Open,
            Some(&project("gitlab.com")),
            [Ok(None), Ok(Some(lists_of_github()))],
            [Ok(None), Ok(None)],
        )
        .unwrap();
        assert_eq!(
            fetched.data.all.unwrap().error.as_deref(),
            Some("glab is not installed or not logged in to gitlab.com")
        );
        // Neither forge usable: still how to log in, with gh missing or
        // not logged in.
        for installed in [[true, false], BOTH] {
            let Err(e) = combine(
                "gitlab.com",
                installed,
                Status::Open,
                None,
                [Ok(None), Ok(None)],
                [Ok(None), Ok(None)],
            ) else {
                panic!("lists without a forge");
            };
            assert!(e.contains("run glab auth login"), "{e}");
            assert_eq!(e.contains("run gh auth login"), installed[1], "{e}");
        }
    }
}
