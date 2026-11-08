//! `b`: fetches the branch of a merge request into the list's own clone and
//! opens it in a new Herdr workspace through Herdr's worktree feature
//! (`herdr worktree create`), which picks the folder and groups it with the
//! repository. The current checkout, its branch and its files are never
//! touched: only the fetched remote ref, branch refs that are not checked out
//! anywhere, and the new folder Herdr makes change.

use crate::cli;
use crate::model::{Forge, MergeRequest};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How long `herdr worktree create` may take.
pub const CREATE_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);

/// Runs one `herdr` command with these arguments and returns its JSON
/// answer. Passed in, so tests can stand in for Herdr.
pub type Herdr<'a> = &'a dyn Fn(&[String]) -> Result<Value, String>;

/// The local branch the merge request opens on: its own branch name when the
/// branch is in the same repository, `pr-<n>` or `mr-<iid>` for a fork, whose
/// branch name may well be taken (a fork's `main`).
pub fn local_branch(mr: &MergeRequest) -> String {
    if !mr.from_fork {
        return mr.source_branch.clone();
    }
    match mr.forge {
        Forge::GitHub => format!("pr-{}", mr.iid),
        Forge::GitLab => format!("mr-{}", mr.iid),
    }
}

/// Where the fetched commit lands: the branch's remote-tracking ref in the
/// same repository, else a ref of the plugin's own that `open` removes
/// again once the local branch holds the commit.
pub fn fetched_ref(mr: &MergeRequest, remote: &str) -> String {
    if mr.from_fork {
        format!("refs/herdr-git-panel/{}", local_branch(mr))
    } else {
        format!("refs/remotes/{remote}/{}", mr.source_branch)
    }
}

/// What `git fetch` asks the remote for, into [`fetched_ref`]: the branch
/// itself in the same repository, else the forge's own ref of the merge
/// request, which the target project keeps for forks too. A ref of its own
/// (not the shared `FETCH_HEAD`) so another fetch at the same time cannot
/// change which commit opens.
pub fn refspec(mr: &MergeRequest, remote: &str) -> String {
    let from = if mr.from_fork {
        match mr.forge {
            Forge::GitHub => format!("refs/pull/{}/head", mr.iid),
            Forge::GitLab => format!("refs/merge-requests/{}/head", mr.iid),
        }
    } else {
        format!("refs/heads/{}", mr.source_branch)
    };
    format!("+{from}:{}", fetched_ref(mr, remote))
}

/// Removes the plugin's own fetch ref of a fork when `open` ends, whichever
/// way it ends.
struct DropRef<'a> {
    dir: &'a Path,
    name: Option<String>,
}

impl Drop for DropRef<'_> {
    fn drop(&mut self) {
        if let Some(name) = &self.name {
            let _ = git(self.dir, &["update-ref", "-d", name], "git");
        }
    }
}

/// Whether a branch name from the forge is safe to hand to `git` and
/// `herdr` as an argument: never an option (`-…`), and nothing git itself
/// refuses in a branch name.
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("/.")
        && !name.contains("@{")
        && name != "@"
        && name != "HEAD"
        && !name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
}

/// A folder where a branch is checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub path: PathBuf,
    /// The repository's main working tree (not a linked worktree).
    pub main: bool,
    /// The folder is gone (git calls the worktree prunable).
    pub gone: bool,
}

/// Where `branch` is checked out, from `git worktree list --porcelain`,
/// whose first entry is the main working tree.
pub fn checked_out(porcelain: &str, branch: &str) -> Option<Checkout> {
    let want = format!("refs/heads/{branch}");
    // Entries are separated by a blank line and start with `worktree <path>`.
    porcelain
        .split("\n\n")
        .filter(|entry| !entry.trim().is_empty())
        .enumerate()
        .find_map(|(index, entry)| {
            let mut lines = entry.lines();
            let path = lines.next()?.strip_prefix("worktree ")?;
            let entry: Vec<&str> = lines.collect();
            entry
                .iter()
                .any(|l| l.strip_prefix("branch ") == Some(want.as_str()))
                .then(|| Checkout {
                    path: PathBuf::from(path),
                    main: index == 0,
                    gone: entry.iter().any(|l| l.starts_with("prunable")),
                })
        })
}

/// `herdr worktree create`: a new folder with `branch` checked out (Herdr
/// checks out an existing local branch), opened as a workspace.
pub fn create_args(dir: &Path, branch: &str) -> Vec<String> {
    vec![
        "worktree".into(),
        "create".into(),
        "--cwd".into(),
        dir.display().to_string(),
        "--branch".into(),
        branch.into(),
        "--focus".into(),
    ]
}

/// `herdr worktree open`: an existing folder, opened (or focused) as a
/// workspace.
pub fn open_args(dir: &Path, path: &Path) -> Vec<String> {
    vec![
        "worktree".into(),
        "open".into(),
        "--cwd".into(),
        dir.display().to_string(),
        "--path".into(),
        path.display().to_string(),
        "--focus".into(),
    ]
}

/// One line from Herdr's error answer (`{"error": {"message": …}}` on
/// stderr), preferring git's own `fatal:` line inside it.
pub fn herdr_error(stderr: &str) -> String {
    let message = serde_json::from_str::<Value>(stderr.trim())
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| stderr.to_string());
    let line = message
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("fatal:"))
        .or_else(|| message.lines().map(str::trim).rfind(|l| !l.is_empty()))
        .unwrap_or("unknown error")
        .trim();
    format!("Herdr could not open the folder: {line}")
}

/// Runs the real `herdr`. Creating a worktree checks out the whole branch,
/// which takes a while in a large repository, so it gets two minutes.
pub fn run_herdr(args: &[String]) -> Result<Value, String> {
    let mut cmd = Command::new(crate::pane::herdr());
    cmd.args(args).stdin(Stdio::null());
    let limit = if args.get(1).map(String::as_str) == Some("create") {
        CREATE_LIMIT
    } else {
        cli::TIMEOUT
    };
    let out = cli::run_for("herdr", cmd, "Herdr", limit)?;
    if !out.success {
        return Err(herdr_error(&out.stderr));
    }
    serde_json::from_slice(&out.stdout).map_err(|_| "Unreadable answer from Herdr".to_string())
}

/// Runs git in `dir` without asking anything: the list owns the terminal.
/// git does not prompt for HTTPS logins, Git Credential Manager shows no
/// window, and ssh asks its "askpass" program instead of the terminal for a
/// passphrase or a new host key, which `false` always declines, so a missing
/// login fails at once instead of waiting. The user's own ssh command and
/// agent still apply. A process group of its own keeps anything else off
/// the terminal. `host` names the server in the message when it takes
/// longer than 30 s.
fn git(dir: &Path, args: &[&str], host: &str) -> Result<cli::Output, String> {
    let mut cmd = Command::new(cli::find("git").unwrap_or_else(|| PathBuf::from("git")));
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("SSH_ASKPASS", "false")
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cli::run("git", cmd, host)
}

/// Runs a local git command that must work; its output, trimmed.
fn git_ok(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(dir, args, "git")?;
    if !out.success {
        return Err(format!("git {} failed: {}", args[0], reason(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The commit a ref points at, or `None` when there is no such ref.
fn commit_of(dir: &Path, name: &str) -> Result<Option<String>, String> {
    let out = git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{name}^{{commit}}"),
        ],
        "git",
    )?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((out.success && !sha.is_empty()).then_some(sha))
}

/// git's own reason, without its `fatal:` or `error:` prefix.
fn reason(stderr: &str) -> String {
    let line = stderr
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .or_else(|| stderr.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("unknown error");
    line.trim_start_matches("fatal:")
        .trim_start_matches("error:")
        .trim()
        .to_string()
}

/// Why the fetch failed, in a line.
fn fetch_failure(mr: &MergeRequest, stderr: &str) -> String {
    let lower = stderr.to_lowercase();
    if lower.contains("couldn't find remote ref") {
        return if mr.from_fork {
            format!(
                "{} has no branch to fetch on {} any more",
                mr.reference(),
                mr.forge.name()
            )
        } else {
            format!(
                "The branch {} is gone on {}",
                mr.source_branch,
                mr.forge.name()
            )
        };
    }
    if lower.contains("could not resolve host")
        || lower.contains("network is unreachable")
        || lower.contains("connection timed out")
        || lower.contains("connection refused")
    {
        return format!("Cannot reach {}: check the connection", mr.host);
    }
    // ssh says why on the line before git's own "Could not read from remote
    // repository" (a missing key, an unknown host key).
    if lower.contains("could not read from remote repository") {
        if let Some(why) = stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
            if !why.starts_with("fatal:") {
                return format!("Could not fetch {}: {why}", mr.reference());
            }
        }
    }
    format!("Could not fetch {}: {}", mr.reference(), reason(stderr))
}

/// Whether two paths name the same folder (git may give one through a
/// symlink, such as `/private/var` for `/var` on macOS).
fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Shortens a path under the home folder to `~/…`.
fn tilde(path: &Path) -> String {
    let shown = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => match shown.strip_prefix(&home) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
            _ => shown,
        },
        _ => shown,
    }
}

/// How the local branch stands against the fetched commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
    Same,
    /// The fetched commit only adds to it: a fast-forward.
    Behind,
    /// It has commits of its own.
    Differs,
}

fn relation(dir: &Path, local: &str, tip: &str) -> Result<Relation, String> {
    if local == tip {
        return Ok(Relation::Same);
    }
    let out = git(dir, &["merge-base", "--is-ancestor", local, tip], "git")?;
    match out.code {
        Some(0) => Ok(Relation::Behind),
        Some(1) => Ok(Relation::Differs),
        _ => Err(format!("git merge-base failed: {}", reason(&out.stderr))),
    }
}

/// Fetches the merge request's branch from `remote` into the clone at `dir`
/// and opens it in a Herdr workspace: the existing folder when the branch is
/// checked out in a linked worktree already, else a new one. Refuses, before
/// fetching, when the branch is checked out in the main working tree or in
/// `dir` itself. On an error nothing but the fetched remote-tracking ref has
/// changed.
pub fn open(mr: &MergeRequest, dir: &Path, remote: &str, herdr: Herdr) -> Result<String, String> {
    let branch = local_branch(mr);
    let noun = mr.forge.noun();

    // Before fetching: a branch checked out in the main working tree or in
    // the list's own folder stays as it is, also its remote-tracking ref.
    let worktrees = git_ok(dir, &["worktree", "list", "--porcelain"])?;
    let checkout = checked_out(&worktrees, &branch);
    if let Some(at) = &checkout {
        let here = git_ok(dir, &["rev-parse", "--show-toplevel"])?;
        if same_folder(&at.path, Path::new(&here)) {
            return Err(format!(
                "{branch} is checked out in this folder already: work here"
            ));
        }
        if at.main {
            return Err(format!(
                "{branch} is checked out in {} already: work there, or switch that folder to another branch",
                tilde(&at.path)
            ));
        }
        if at.gone {
            return Err(format!(
                "{branch} is still checked out in {}, which is gone: run git worktree prune",
                tilde(&at.path)
            ));
        }
    }

    let fetched = fetched_ref(mr, remote);
    let _cleanup = DropRef {
        dir,
        name: mr.from_fork.then(|| fetched.clone()),
    };
    let out = git(
        dir,
        &[
            "fetch",
            "--no-tags",
            "--recurse-submodules=no",
            "--quiet",
            remote,
            &refspec(mr, remote),
        ],
        &mr.host,
    )?;
    if !out.success {
        return Err(fetch_failure(mr, &out.stderr));
    }
    let tip = commit_of(dir, &fetched)?
        .ok_or_else(|| format!("Could not fetch {}: no commit arrived", mr.reference()))?;

    let existing = commit_of(dir, &format!("refs/heads/{branch}"))?;
    let relation = match &existing {
        Some(local) => Some(relation(dir, local, &tip)?),
        None => None,
    };

    // Checked out in a linked worktree already: open that folder again and
    // leave its files alone.
    if let Some(at) = checkout {
        herdr(&open_args(dir, &at.path))?;
        let note = match relation {
            Some(Relation::Behind) => format!("its branch is behind the {noun}"),
            Some(Relation::Differs) => format!("its branch differs from the {noun}"),
            _ => String::new(),
        };
        return Ok(finish(
            herdr,
            format!("Opened {} again", tilde(&at.path)),
            &note,
        ));
    }

    // The branch is checked out nowhere: create it, or move it forward.
    let local_ref = format!("refs/heads/{branch}");
    let undo: Option<Vec<String>> = match (&existing, relation) {
        (None, _) => {
            git_ok(dir, &["branch", "--no-track", &branch, &tip])?;
            Some(vec!["branch".into(), "-D".into(), branch.clone()])
        }
        (Some(old), Some(Relation::Behind)) => {
            git_ok(dir, &["update-ref", &local_ref, &tip, old])?;
            Some(vec![
                "update-ref".into(),
                local_ref.clone(),
                old.clone(),
                tip.clone(),
            ])
        }
        _ => None,
    };

    let answer = match herdr(&create_args(dir, &branch)) {
        Ok(answer) => answer,
        Err(e) => {
            // Undo only while the branch is checked out nowhere: Herdr may
            // have made the folder before it failed (or ran out of time), and
            // moving a checked-out branch would leave that folder broken.
            let still_free = git_ok(dir, &["worktree", "list", "--porcelain"])
                .is_ok_and(|list| checked_out(&list, &branch).is_none());
            if let (Some(args), true) = (undo, still_free) {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let _ = git_ok(dir, &args);
            }
            return Err(e);
        }
    };

    // Track the merge request's branch, so `git push` there updates it. Set
    // through the config, which works whatever the remote's fetch refspec,
    // and only on a branch that has no upstream and no commits of its own.
    let mut note = String::new();
    if relation == Some(Relation::Differs) {
        note = format!("the local branch {branch} differs from the {noun}, kept it as it is");
        if mr.from_fork {
            note.push_str(" (delete it to open the current version)");
        }
    } else if !mr.from_fork && !has_upstream(dir, &branch) {
        let set =
            git_ok(dir, &["config", &format!("branch.{branch}.remote"), remote]).and_then(|_| {
                git_ok(
                    dir,
                    &[
                        "config",
                        &format!("branch.{branch}.merge"),
                        &format!("refs/heads/{}", mr.source_branch),
                    ],
                )
            });
        if set.is_err() {
            note = format!(
                "could not make {branch} track {remote}/{}",
                mr.source_branch
            );
        }
    }
    let place = answer["result"]["worktree"]["path"]
        .as_str()
        .map(|p| tilde(Path::new(p)))
        .unwrap_or_else(|| "a new workspace".to_string());
    Ok(finish(herdr, format!("Opened {branch} in {place}"), &note))
}

/// Whether the local branch has an upstream already.
fn has_upstream(dir: &Path, branch: &str) -> bool {
    git(
        dir,
        &["config", "--get", &format!("branch.{branch}.merge")],
        "git",
    )
    .is_ok_and(|out| out.success)
}

/// The message for the list, with the note after it. Herdr has just
/// switched to the workspace, so the list is out of sight: a note is also
/// shown as a Herdr notification there.
fn finish(herdr: Herdr, message: String, note: &str) -> String {
    if note.is_empty() {
        return message;
    }
    let _ = herdr(&notify_args(&message, note));
    format!("{message}; {note}")
}

/// `herdr notification show`: a toast in the workspace that is in front.
pub fn notify_args(title: &str, body: &str) -> Vec<String> {
    vec![
        "notification".into(),
        "show".into(),
        title.into(),
        "--body".into(),
        body.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MergeState, Pipeline};
    use std::cell::RefCell;

    /// A made-up merge request of `acme/app`.
    fn mr(forge: Forge, iid: u64, branch: &str, from_fork: bool) -> MergeRequest {
        MergeRequest {
            forge,
            host: "github.com".into(),
            viewer: "me".into(),
            iid,
            title: "Add export".into(),
            description: String::new(),
            url: String::new(),
            project: "acme/app".into(),
            author: "alex".into(),
            source_branch: branch.into(),
            from_fork,
            head_sha: String::new(),
            target_branch: "main".into(),
            created: None,
            updated: None,
            merge_state: MergeState::Blocked,
            status: crate::model::Status::Open,
            approvals_given: 0,
            approvals_required: None,
            approved_by: vec![],
            approved: false,
            pending_rules: vec![],
            reviewers: vec![],
            threads_total: 0,
            threads_open: 0,
            labels: vec![],
            pipeline: Pipeline::None,
            additions: None,
            deletions: None,
            merge_method: String::new(),
        }
    }

    /// git for setting up a test, untouched by the user's own config.
    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A bare "remote" and a clone of it, in a fresh temp folder.
    struct Setup {
        root: PathBuf,
        remote: PathBuf,
        clone: PathBuf,
        /// The first commit, `main` everywhere.
        base: String,
    }

    impl Setup {
        fn new(name: &str) -> Setup {
            let root = std::env::temp_dir().join(format!(
                "herdr-git-panel-branch-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let remote = root.join("remote.git");
            sh(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
            let mut s = Setup {
                clone: root.join("clone"),
                root,
                remote,
                base: String::new(),
            };
            s.base = s.commit(None, "first");
            sh(&s.remote, &["update-ref", "refs/heads/main", &s.base]);
            sh(&s.root, &["clone", "-q", "remote.git", "clone"]);
            s
        }

        /// A new commit in the remote, on top of `parent`.
        fn commit(&self, parent: Option<&str>, message: &str) -> String {
            let tree = sh(
                &self.remote,
                &["hash-object", "-t", "tree", "-w", "/dev/null"],
            );
            let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
            if let Some(p) = parent {
                args.extend(["-p", p]);
            }
            sh(&self.remote, &args)
        }

        /// A branch (or other ref) in the remote.
        fn remote_ref(&self, name: &str, commit: &str) {
            sh(&self.remote, &["update-ref", name, commit]);
        }

        fn local(&self, name: &str) -> Option<String> {
            commit_of(&self.clone, &format!("refs/heads/{name}")).unwrap()
        }
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Stands in for Herdr: records each call, and `worktree create` adds a
    /// git worktree next to the clone, as Herdr does in its own folder.
    struct FakeHerdr {
        calls: RefCell<Vec<Vec<String>>>,
        root: PathBuf,
        refuse: bool,
        /// Makes the folder, then fails (as when Herdr runs out of time).
        fail_after_create: bool,
    }

    impl FakeHerdr {
        fn new(s: &Setup) -> FakeHerdr {
            FakeHerdr {
                calls: RefCell::new(Vec::new()),
                root: s.root.clone(),
                refuse: false,
                fail_after_create: false,
            }
        }

        /// The notifications shown, as (title, body).
        fn notifications(&self) -> Vec<(String, String)> {
            self.calls
                .borrow()
                .iter()
                .filter(|c| c[0] == "notification")
                .map(|c| (c[2].clone(), c[4].clone()))
                .collect()
        }

        fn run(&self, args: &[String]) -> Result<Value, String> {
            self.calls.borrow_mut().push(args.to_vec());
            if self.refuse {
                return Err("Herdr could not open the folder: no".into());
            }
            if args[1] == "create" {
                let dir = PathBuf::from(&args[3]);
                let branch = &args[5];
                let path = self.root.join(format!("wt-{branch}"));
                sh(
                    &dir,
                    &["worktree", "add", "-q", path.to_str().unwrap(), branch],
                );
                if self.fail_after_create {
                    return Err("Herdr did not answer within 120 s".into());
                }
                return Ok(serde_json::json!({"result": {"worktree": {"path": path}}}));
            }
            Ok(serde_json::json!({"result": {}}))
        }
    }

    fn open_with(s: &Setup, mr: &MergeRequest, herdr: &FakeHerdr) -> Result<String, String> {
        open(mr, &s.clone, "origin", &|a: &[String]| herdr.run(a))
    }

    #[test]
    fn opens_a_new_folder_on_the_forges_head_that_tracks_the_branch() {
        let s = Setup::new("new");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        let herdr = FakeHerdr::new(&s);

        let msg = open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap();

        assert!(msg.starts_with("Opened export in "), "{msg}");
        assert_eq!(*herdr.calls.borrow(), vec![create_args(&s.clone, "export")]);
        let folder = s.root.join("wt-export");
        assert_eq!(sh(&folder, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            sh(&folder, &["rev-parse", "--abbrev-ref", "export@{upstream}"]),
            "origin/export"
        );
    }

    #[test]
    fn the_current_checkout_keeps_its_branch_and_changes() {
        let s = Setup::new("untouched");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        std::fs::write(s.clone.join("notes.txt"), "not committed\n").unwrap();
        sh(&s.clone, &["add", "notes.txt"]);
        std::fs::write(s.clone.join("scratch.txt"), "untracked\n").unwrap();
        let status = sh(&s.clone, &["status", "--porcelain", "--branch"]);
        let herdr = FakeHerdr::new(&s);

        open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap();

        assert_eq!(sh(&s.clone, &["status", "--porcelain", "--branch"]), status);
        assert_eq!(sh(&s.clone, &["rev-parse", "HEAD"]), s.base);
        assert_eq!(sh(&s.clone, &["branch", "--show-current"]), "main");
    }

    #[test]
    fn a_second_time_opens_the_same_folder_again() {
        let s = Setup::new("again");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        let herdr = FakeHerdr::new(&s);
        let m = mr(Forge::GitHub, 3, "export", false);

        open_with(&s, &m, &herdr).unwrap();
        let msg = open_with(&s, &m, &herdr).unwrap();

        let folder = s.root.join("wt-export");
        assert!(msg.contains("again"), "{msg}");
        let calls = herdr.calls.borrow();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1][..4], open_args(&s.clone, &folder)[..4]);
        // git may name the folder by its real path (/private/var on macOS).
        let opened = std::fs::canonicalize(&calls[1][5]).unwrap();
        assert_eq!(opened, std::fs::canonicalize(&folder).unwrap());
        let worktrees = sh(&s.clone, &["worktree", "list", "--porcelain"]);
        assert_eq!(worktrees.matches("worktree ").count(), 2, "{worktrees}");
    }

    #[test]
    fn a_fork_opens_its_own_commits_even_when_its_branch_is_called_main() {
        let s = Setup::new("fork");
        let fork_head = s.commit(None, "fork work");
        // The pull request's own ref, which the target repository keeps.
        s.remote_ref("refs/pull/7/head", &fork_head);
        let herdr = FakeHerdr::new(&s);

        let msg = open_with(&s, &mr(Forge::GitHub, 7, "main", true), &herdr).unwrap();

        assert!(msg.starts_with("Opened pr-7 in "), "{msg}");
        assert_eq!(s.local("pr-7"), Some(fork_head.clone()));
        assert_eq!(s.local("main"), Some(s.base.clone()), "own main untouched");
        let folder = s.root.join("wt-pr-7");
        assert_eq!(sh(&folder, &["rev-parse", "HEAD"]), fork_head);
        let upstream = Command::new("git")
            .arg("-C")
            .arg(&folder)
            .args(["rev-parse", "--abbrev-ref", "pr-7@{upstream}"])
            .output()
            .unwrap();
        assert!(!upstream.status.success(), "a fork's branch tracks nothing");
        assert_eq!(
            commit_of(&s.clone, "refs/herdr-git-panel/pr-7").unwrap(),
            None,
            "the plugin's own fetch ref is gone again"
        );
    }

    #[test]
    fn the_commit_comes_from_the_fetch_not_from_fetch_head() {
        let s = Setup::new("fetch-head");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        // Another fetch's FETCH_HEAD, written before ours reads it.
        std::fs::write(
            s.clone.join(".git/FETCH_HEAD"),
            format!("{}\t\tbranch 'main' of x\n", s.base),
        )
        .unwrap();
        sh(&s.clone, &["config", "fetch.writeFetchHead", "false"]);
        let herdr = FakeHerdr::new(&s);

        open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap();

        assert_eq!(s.local("export"), Some(head));
    }

    #[test]
    fn forks_use_the_forges_own_ref() {
        let gh = mr(Forge::GitHub, 7, "main", true);
        assert_eq!(local_branch(&gh), "pr-7");
        assert_eq!(
            refspec(&gh, "origin"),
            "+refs/pull/7/head:refs/herdr-git-panel/pr-7"
        );
        let gl = mr(Forge::GitLab, 42, "main", true);
        assert_eq!(local_branch(&gl), "mr-42");
        assert_eq!(
            refspec(&gl, "origin"),
            "+refs/merge-requests/42/head:refs/herdr-git-panel/mr-42"
        );
        let same = mr(Forge::GitLab, 42, "fix/login", false);
        assert_eq!(local_branch(&same), "fix/login");
        assert_eq!(
            refspec(&same, "upstream"),
            "+refs/heads/fix/login:refs/remotes/upstream/fix/login"
        );
    }

    #[test]
    fn a_local_branch_with_own_commits_is_kept() {
        let s = Setup::new("differs");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        let mine = s.commit(Some(&s.base), "my own work");
        sh(&s.clone, &["fetch", "-q", "origin", &mine]);
        sh(&s.clone, &["branch", "export", &mine]);
        let herdr = FakeHerdr::new(&s);

        let msg = open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap();

        assert!(msg.contains("differs"), "{msg}");
        assert_eq!(s.local("export"), Some(mine));
        assert!(!has_upstream(&s.clone, "export"), "its config is kept too");
        // The list is out of sight now: Herdr shows the note.
        let shown = herdr.notifications();
        assert_eq!(shown.len(), 1);
        assert!(shown[0].1.contains("differs"), "{shown:?}");
    }

    #[test]
    fn a_rebased_fork_says_how_to_get_the_current_version() {
        let s = Setup::new("rebased");
        let first = s.commit(None, "fork work");
        s.remote_ref("refs/pull/7/head", &first);
        let herdr = FakeHerdr::new(&s);
        let m = mr(Forge::GitHub, 7, "main", true);
        open_with(&s, &m, &herdr).unwrap();
        sh(
            &s.clone,
            &[
                "worktree",
                "remove",
                s.root.join("wt-pr-7").to_str().unwrap(),
            ],
        );
        let rebased = s.commit(None, "fork work, rebased");
        s.remote_ref("refs/pull/7/head", &rebased);

        let msg = open_with(&s, &m, &herdr).unwrap();

        assert!(
            msg.contains("delete it to open the current version"),
            "{msg}"
        );
        assert_eq!(s.local("pr-7"), Some(first));
    }

    #[test]
    fn a_plain_open_shows_no_notification() {
        let s = Setup::new("quiet");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        let herdr = FakeHerdr::new(&s);
        open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap();
        assert!(herdr.notifications().is_empty());
    }

    #[test]
    fn a_local_branch_behind_the_merge_request_moves_forward() {
        let s = Setup::new("behind");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        sh(&s.clone, &["branch", "export", &s.base]);
        let herdr = FakeHerdr::new(&s);

        let msg = open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap();

        assert!(!msg.contains("differs"), "{msg}");
        assert_eq!(s.local("export"), Some(head));
    }

    #[test]
    fn a_branch_checked_out_in_the_main_checkout_is_refused() {
        let s = Setup::new("main-checkout");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        sh(&s.clone, &["fetch", "-q", "origin"]);
        sh(&s.clone, &["switch", "-q", "export"]);
        let before = s.local("export");
        // Pushed after the clone fetched: its remote-tracking ref stays old.
        let newer = s.commit(Some(&head), "more");
        s.remote_ref("refs/heads/export", &newer);
        let herdr = FakeHerdr::new(&s);
        let m = mr(Forge::GitHub, 3, "export", false);
        // The list runs in another folder of the same repository.
        let side = s.root.join("side");
        sh(
            &s.clone,
            &["worktree", "add", "-q", "--detach", side.to_str().unwrap()],
        );

        let e = open(&m, &side, "origin", &|a: &[String]| herdr.run(a)).unwrap_err();

        assert!(e.contains("is checked out in"), "{e}");
        assert!(e.contains("clone"), "names the folder: {e}");
        // The list runs in the main checkout itself.
        let e = open_with(&s, &m, &herdr).unwrap_err();
        assert_eq!(e, "export is checked out in this folder already: work here");
        assert!(herdr.calls.borrow().is_empty());
        assert_eq!(s.local("export"), before);
        assert_eq!(sh(&s.clone, &["branch", "--show-current"]), "export");
        assert_eq!(sh(&s.clone, &["rev-parse", "origin/export"]), head);
    }

    #[test]
    fn a_folder_deleted_by_hand_is_named() {
        let s = Setup::new("deleted");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        let herdr = FakeHerdr::new(&s);
        let m = mr(Forge::GitHub, 3, "export", false);
        open_with(&s, &m, &herdr).unwrap();
        std::fs::remove_dir_all(s.root.join("wt-export")).unwrap();

        let e = open_with(&s, &m, &herdr).unwrap_err();

        assert!(e.contains("which is gone: run git worktree prune"), "{e}");
        assert_eq!(herdr.calls.borrow().len(), 1, "Herdr not asked again");
    }

    #[test]
    fn a_deleted_branch_changes_nothing() {
        let s = Setup::new("gone");
        let herdr = FakeHerdr::new(&s);

        let e = open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap_err();

        assert_eq!(e, "The branch export is gone on GitHub");
        assert_eq!(s.local("export"), None);
        assert!(herdr.calls.borrow().is_empty());
        let e = open_with(&s, &mr(Forge::GitHub, 8, "x", true), &herdr).unwrap_err();
        assert_eq!(e, "#8 has no branch to fetch on GitHub any more");
    }

    #[test]
    fn an_unreachable_remote_changes_nothing() {
        let s = Setup::new("unreachable");
        sh(
            &s.clone,
            &[
                "remote",
                "set-url",
                "origin",
                s.root.join("nowhere.git").to_str().unwrap(),
            ],
        );
        let herdr = FakeHerdr::new(&s);

        let e = open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap_err();

        assert!(e.starts_with("Could not fetch #3: "), "{e}");
        assert_eq!(s.local("export"), None);
        assert!(herdr.calls.borrow().is_empty());
    }

    #[test]
    fn a_refusal_by_herdr_takes_the_branch_back() {
        let s = Setup::new("refused");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        s.remote_ref("refs/heads/later", &head);
        sh(&s.clone, &["branch", "later", &s.base]);
        let mut herdr = FakeHerdr::new(&s);
        herdr.refuse = true;

        let e = open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap_err();
        assert!(e.starts_with("Herdr could not open"), "{e}");
        assert_eq!(s.local("export"), None, "the new branch is gone again");

        open_with(&s, &mr(Forge::GitHub, 4, "later", false), &herdr).unwrap_err();
        assert_eq!(s.local("later"), Some(s.base.clone()), "moved back");
    }

    #[test]
    fn a_folder_herdr_made_before_failing_keeps_its_branch() {
        let s = Setup::new("late");
        let head = s.commit(Some(&s.base), "export");
        s.remote_ref("refs/heads/export", &head);
        s.remote_ref("refs/heads/later", &head);
        sh(&s.clone, &["branch", "later", &s.base]);
        let mut herdr = FakeHerdr::new(&s);
        herdr.fail_after_create = true;

        let e = open_with(&s, &mr(Forge::GitHub, 3, "export", false), &herdr).unwrap_err();
        assert!(e.contains("did not answer"), "{e}");
        assert_eq!(
            s.local("export"),
            Some(head.clone()),
            "not deleted under the folder"
        );

        open_with(&s, &mr(Forge::GitHub, 4, "later", false), &herdr).unwrap_err();
        assert_eq!(
            s.local("later"),
            Some(head.clone()),
            "not moved back under the folder"
        );
        let folder = s.root.join("wt-later");
        assert_eq!(
            sh(&folder, &["status", "--porcelain"]),
            "",
            "the folder is clean"
        );
    }

    #[test]
    fn worktree_list_names_where_a_branch_is() {
        let list = "worktree /src/app\nHEAD 1111\nbranch refs/heads/main\n\n\
                    worktree /wt/app/export\nHEAD 2222\nbranch refs/heads/export\n\n\
                    worktree /wt/app/old\nHEAD 3333\ndetached\n";
        assert_eq!(
            checked_out(list, "main"),
            Some(Checkout {
                path: "/src/app".into(),
                main: true,
                gone: false
            })
        );
        assert_eq!(
            checked_out(list, "export"),
            Some(Checkout {
                path: "/wt/app/export".into(),
                main: false,
                gone: false
            })
        );
        assert_eq!(checked_out(list, "exp"), None);
        let gone = "worktree /src/app\nHEAD 1111\nbranch refs/heads/main\n\n\
                    worktree /wt/app/old\nHEAD 3333\nbranch refs/heads/old\nprunable gitdir file points to non-existent location\n";
        assert!(checked_out(gone, "old").unwrap().gone);
        assert_eq!(checked_out("worktree /bare\nbare\n", "main"), None);
    }

    #[test]
    fn branch_names_that_could_be_options_are_refused() {
        for ok in ["main", "fix/login", "release-1.2", "alex/add_export"] {
            assert!(valid_branch(ok), "{ok}");
        }
        for bad in [
            "",
            "-x",
            "--upload-pack=touch",
            "a b",
            "a..b",
            "a:b",
            "x\n",
            "/x",
            "x/",
            "x.lock",
            "a/.b",
            "@",
            "HEAD",
            "a@{1}",
        ] {
            assert!(!valid_branch(bad), "{bad:?}");
        }
    }

    #[test]
    fn herdr_errors_show_gits_reason() {
        let stderr = r#"{"error":{"code":"worktree_create_failed","message":"Preparing worktree (checking out 'main')\nfatal: 'main' is already used by worktree at '/src/app'"},"id":"cli:worktree:create"}"#;
        assert_eq!(
            herdr_error(stderr),
            "Herdr could not open the folder: 'main' is already used by worktree at '/src/app'"
        );
        assert_eq!(
            herdr_error("{\"error\":{\"message\":\"no such workspace\"}}"),
            "Herdr could not open the folder: no such workspace"
        );
        assert_eq!(
            herdr_error("something odd\n"),
            "Herdr could not open the folder: something odd"
        );
    }
}
