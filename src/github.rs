//! GitHub: open pull requests, their review threads, approve, merge and
//! changes, through the `gh` CLI.

use crate::cli;
use crate::model::*;
use crate::time;
use serde_json::Value;

pub const HOST: &str = "github.com";

/// Pull requests waiting for your review, ones you already reviewed (GitHub
/// drops the review request once you review, GitLab keeps it), and your own.
/// One request each, at the same time: in one request GitHub needs too long
/// to work out mergeability and answers 502.
const SEARCHES: [(&str, &str); 3] = [
    ("requested", "review-requested:@me"),
    ("reviewed", "reviewed-by:@me -author:@me"),
    ("mine", "author:@me"),
];

/// `KEY` and `TERMS` are replaced per search.
const QUERY: &str = r#"
query {
  viewer { login }
  KEY: search(query: "is:open is:pr archived:false TERMS", type: ISSUE, first: 30) { nodes { ...pr } }
}
fragment pr on PullRequest {
  number title body url isDraft createdAt updatedAt headRefName baseRefName headRefOid
  repository { nameWithOwner viewerDefaultMergeMethod }
  author { login }
  reviewDecision mergeable mergeStateStatus
  baseRef { branchProtectionRule { requiredApprovingReviewCount } }
  latestReviews(first: 20) { nodes { author { login } state } }
  reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } ... on Team { slug } } } }
  reviewThreads(first: 50) { nodes { isResolved } }
  labels(first: 20) { nodes { name color } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
  additions deletions
}
"#;

/// Whether gh is installed at all; without it GitHub is left out quietly.
pub fn available() -> bool {
    cli::find("gh").is_some()
}

/// Your pull requests and those you review. `Ok(None)` when gh is not logged
/// in: then GitHub is simply not used.
pub fn fetch() -> Result<Option<Data>, String> {
    let answers: Vec<Result<Value, Failure>> = std::thread::scope(|scope| {
        let runs: Vec<_> = SEARCHES
            .iter()
            .map(|(key, terms)| {
                scope
                    .spawn(move || graphql(&QUERY.replace("KEY", key).replace("TERMS", terms), &[]))
            })
            .collect();
        runs.into_iter()
            .map(|r| {
                r.join()
                    .unwrap_or_else(|_| Err(Failure::Other("gh crashed".into())))
            })
            .collect()
    });
    // One answer with all three lists, as if it were one request.
    let mut merged = serde_json::json!({"data": {}});
    for answer in answers {
        let value = match answer {
            Ok(v) => v,
            Err(Failure::NotLoggedIn) => return Ok(None),
            Err(Failure::Other(e)) => return Err(e),
        };
        if cli::graphql_errors(&value).is_some() {
            return parse(&value).map(Some);
        }
        if let (Some(into), Some(from)) = (
            merged["data"].as_object_mut(),
            value.get("data").and_then(Value::as_object),
        ) {
            into.extend(from.clone());
        }
    }
    parse(&merged).map(Some)
}

enum Failure {
    NotLoggedIn,
    Other(String),
}

impl From<Failure> for String {
    fn from(f: Failure) -> String {
        match f {
            Failure::NotLoggedIn => format!("gh is not logged in to {HOST}: run gh auth login"),
            Failure::Other(e) => e,
        }
    }
}

/// Runs a GraphQL query through gh. Variables starting with `$int:` are sent
/// as numbers.
fn graphql(query: &str, vars: &[(&str, &str)]) -> Result<Value, Failure> {
    let mut cmd = cli::command("gh");
    cmd.args(["api", "graphql", "--hostname", HOST, "-f"])
        .arg(format!("query={query}"));
    for (k, v) in vars {
        match v.strip_prefix("$int:") {
            Some(n) => cmd.arg("-F").arg(format!("{k}={n}")),
            None => cmd.arg("-f").arg(format!("{k}={v}")),
        };
    }
    let out = cli::run("gh", cmd, HOST).map_err(Failure::Other)?;
    if !out.success && cli::not_logged_in(&out.stderr) {
        return Err(Failure::NotLoggedIn);
    }
    // GitHub's own errors come as JSON, also when gh exits with an error.
    if let Ok(value) = serde_json::from_slice::<Value>(&out.stdout) {
        return Ok(value);
    }
    if !out.success {
        return Err(Failure::Other(cli::explain_failure(
            "gh",
            HOST,
            &out.stderr,
        )));
    }
    Err(Failure::Other("unreadable answer from GitHub".into()))
}

/// Runs a `gh pr …` command on the pull request and returns its output.
fn pr_command(mr: &MergeRequest, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = cli::command("gh");
    cmd.arg("pr")
        .args(&args[..1])
        .arg(mr.iid.to_string())
        .args(&args[1..])
        .args(["--repo", &mr.project]);
    let out = cli::run("gh", cmd, HOST)?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(cli::explain_failure("gh", HOST, &out.stderr))
    }
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

pub fn parse(value: &Value) -> Result<Data, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitHub: {msg}"));
    }
    let data = &value["data"];
    let viewer = str_of(&data["viewer"]["login"]);
    if viewer.is_empty() {
        return Err("GitHub did not return a user: run gh auth login".into());
    }
    let list = |keys: &[&str]| -> Vec<MergeRequest> {
        let mut prs: Vec<MergeRequest> = Vec::new();
        for key in keys {
            for n in data[*key]["nodes"].as_array().into_iter().flatten() {
                if let Some(pr) = parse_pr(n, &viewer) {
                    if !prs.iter().any(|p| p.key() == pr.key()) {
                        prs.push(pr);
                    }
                }
            }
        }
        prs.sort_by_key(|m| std::cmp::Reverse(m.updated));
        prs
    };
    Ok(Data {
        review: list(&["requested", "reviewed"]),
        mine: list(&["mine"]),
    })
}

fn parse_pr(n: &Value, viewer: &str) -> Option<MergeRequest> {
    // Search results that are not pull requests come back as empty objects.
    let number = n["number"].as_u64()?;
    let author = str_of(&n["author"]["login"]);

    let merge_state = if n["isDraft"].as_bool().unwrap_or(false) {
        MergeState::Draft
    } else if n["mergeable"].as_str() == Some("CONFLICTING") {
        MergeState::Conflict
    } else {
        match n["mergeStateStatus"].as_str().unwrap_or("") {
            "CLEAN" | "HAS_HOOKS" | "UNSTABLE" => MergeState::Ready,
            "DIRTY" => MergeState::Conflict,
            "BEHIND" => MergeState::NeedsRebase,
            "DRAFT" => MergeState::Draft,
            "UNKNOWN" | "" => MergeState::Checking,
            _ => MergeState::Blocked,
        }
    };

    // Reviewers: who reviewed (latest review each) and who is still asked.
    let mut reviewers: Vec<Reviewer> = Vec::new();
    for r in n["latestReviews"]["nodes"].as_array().into_iter().flatten() {
        let username = str_of(&r["author"]["login"]);
        if username.is_empty() || username == author {
            continue;
        }
        let state = match r["state"].as_str().unwrap_or("") {
            "APPROVED" => ReviewState::Approved,
            "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
            "COMMENTED" => ReviewState::Reviewed,
            "PENDING" => ReviewState::Started,
            _ => ReviewState::Unreviewed,
        };
        reviewers.push(Reviewer { username, state });
    }
    for r in n["reviewRequests"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let who = &r["requestedReviewer"];
        let username = who["login"]
            .as_str()
            .map(str::to_string)
            .or_else(|| who["slug"].as_str().map(|s| format!("@{s}")))
            .unwrap_or_default();
        if username.is_empty() {
            continue;
        }
        // Asked again after a review: the old review no longer counts.
        reviewers.retain(|x| x.username != username);
        reviewers.push(Reviewer {
            username,
            state: ReviewState::Unreviewed,
        });
    }
    let approved_by: Vec<String> = reviewers
        .iter()
        .filter(|r| r.state == ReviewState::Approved)
        .map(|r| r.username.clone())
        .collect();
    let decision = n["reviewDecision"].as_str().unwrap_or("");
    let approved = decision == "APPROVED" || (decision.is_empty() && !approved_by.is_empty());
    let pending_rules = match decision {
        "REVIEW_REQUIRED" => vec!["required reviews".to_string()],
        "CHANGES_REQUESTED" => vec!["changes requested".to_string()],
        _ => Vec::new(),
    };

    let threads: Vec<bool> = n["reviewThreads"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| t["isResolved"].as_bool().unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    let pipeline = match n["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"].as_str() {
        None => Pipeline::None,
        Some("SUCCESS") => Pipeline::Success,
        Some("FAILURE") | Some("ERROR") => Pipeline::Failed,
        Some(_) => Pipeline::Running,
    };
    let labels = n["labels"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| Label {
                    title: str_of(&l["name"]),
                    color: format!("#{}", str_of(&l["color"])),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(MergeRequest {
        forge: Forge::GitHub,
        host: HOST.to_string(),
        viewer: viewer.to_string(),
        iid: number,
        title: str_of(&n["title"]),
        description: str_of(&n["body"]),
        url: str_of(&n["url"]),
        project: str_of(&n["repository"]["nameWithOwner"]),
        author,
        source_branch: str_of(&n["headRefName"]),
        head_sha: str_of(&n["headRefOid"]),
        target_branch: str_of(&n["baseRefName"]),
        created: n["createdAt"].as_str().and_then(time::parse_iso),
        updated: n["updatedAt"].as_str().and_then(time::parse_iso),
        merge_state,
        approvals_given: approved_by.len() as u64,
        approvals_required: n["baseRef"]["branchProtectionRule"]["requiredApprovingReviewCount"]
            .as_u64(),
        approved_by,
        approved,
        pending_rules,
        reviewers,
        threads_total: threads.len() as u64,
        threads_open: threads.iter().filter(|r| !**r).count() as u64,
        labels,
        pipeline,
        additions: n["additions"].as_u64(),
        deletions: n["deletions"].as_u64(),
        merge_method: n["repository"]["viewerDefaultMergeMethod"]
            .as_str()
            .unwrap_or("MERGE")
            .to_lowercase(),
    })
}

const ACTIVITY: &str = r#"
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      comments(last: 100) { nodes { body createdAt author { login } } }
      reviews(last: 50) { nodes { body state submittedAt author { login } } }
      reviewThreads(first: 100) {
        nodes {
          id isResolved path line originalLine
          comments(first: 50) { nodes { body createdAt author { login } } }
        }
      }
    }
  }
}
"#;

/// Comments, reviews and review threads of a pull request, newest activity
/// first.
pub fn fetch_activity(mr: &MergeRequest) -> Result<Vec<Discussion>, String> {
    let (owner, name) = mr
        .project
        .split_once('/')
        .ok_or("not a GitHub repository")?;
    let number = format!("$int:{}", mr.iid);
    let value = graphql(
        ACTIVITY,
        &[("owner", owner), ("name", name), ("number", &number)],
    )?;
    parse_activity(&value)
}

fn note(n: &Value, at: &str) -> Note {
    Note {
        author: str_of(&n["author"]["login"]),
        body: str_of(&n["body"]),
        created: n[at].as_str().and_then(time::parse_iso),
        system: false,
    }
}

pub fn parse_activity(value: &Value) -> Result<Vec<Discussion>, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitHub: {msg}"));
    }
    let pr = value
        .pointer("/data/repository/pullRequest")
        .filter(|v| !v.is_null())
        .ok_or("GitHub returned no pull request")?;
    let single = |n: Note| Discussion {
        id: String::new(),
        resolvable: false,
        resolved: false,
        place: None,
        notes: vec![n],
    };
    let mut list: Vec<Discussion> = Vec::new();
    for c in pr["comments"]["nodes"].as_array().into_iter().flatten() {
        list.push(single(note(c, "createdAt")));
    }
    for r in pr["reviews"]["nodes"].as_array().into_iter().flatten() {
        let mut n = note(r, "submittedAt");
        if n.body.trim().is_empty() {
            // A review without text: say what it was, like GitLab's own notes.
            n.body = match r["state"].as_str().unwrap_or("") {
                "APPROVED" => "approved this pull request",
                "CHANGES_REQUESTED" => "requested changes",
                "DISMISSED" => "had a review dismissed",
                // Line comments of a review show up in their threads.
                _ => continue,
            }
            .to_string();
            n.system = true;
        }
        list.push(single(n));
    }
    for t in pr["reviewThreads"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let notes: Vec<Note> = t["comments"]["nodes"]
            .as_array()
            .map(|a| a.iter().map(|c| note(c, "createdAt")).collect())
            .unwrap_or_default();
        let place = t["path"].as_str().map(|path| {
            match t["line"].as_u64().or_else(|| t["originalLine"].as_u64()) {
                Some(line) => format!("{path}:{line}"),
                None => path.to_string(),
            }
        });
        list.push(Discussion {
            id: str_of(&t["id"]),
            resolvable: true,
            resolved: t["isResolved"].as_bool().unwrap_or(false),
            place,
            notes,
        });
    }
    list.retain(|d| !d.notes.is_empty());
    list.sort_by_key(|d| std::cmp::Reverse(d.notes.iter().filter_map(|n| n.created).max()));
    Ok(list)
}

/// Marks a review thread as resolved, or opens it again.
pub fn set_resolved(thread: &str, resolve: bool) -> Result<(), String> {
    let field = if resolve {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    };
    let mutation = format!(
        "mutation($id: ID!) {{ {field}(input: {{threadId: $id}}) {{ thread {{ isResolved }} }} }}"
    );
    let value = graphql(&mutation, &[("id", thread)])?;
    match cli::graphql_errors(&value) {
        Some(msg) => Err(format!("GitHub: {msg}")),
        None => Ok(()),
    }
}

/// Merges the pull request with the repository's default method, only if
/// its head is still the one the list showed.
pub fn merge(mr: &MergeRequest) -> Result<(), String> {
    let method = match mr.merge_method.as_str() {
        "squash" => "--squash",
        "rebase" => "--rebase",
        _ => "--merge",
    };
    let mut args = vec!["merge", method];
    if !mr.head_sha.is_empty() {
        args.extend(["--match-head-commit", mr.head_sha.as_str()]);
    }
    pr_command(mr, &args).map(|_| ())
}

/// Approves the pull request. GitHub has no way to take an approval back
/// other than a dismissal by a maintainer.
pub fn set_draft(mr: &MergeRequest, draft: bool) -> Result<(), String> {
    let args: &[&str] = if draft {
        &["ready", "--undo"]
    } else {
        &["ready"]
    };
    pr_command(mr, args).map(|_| ())
}

pub fn approve(mr: &MergeRequest) -> Result<(), String> {
    pr_command(mr, &["review", "--approve"]).map(|_| ())
}

/// The pull request's changes as a unified diff, without colours.
pub fn fetch_diff(mr: &MergeRequest) -> Result<String, String> {
    pr_command(mr, &["diff", "--color", "never"])
        .map(|out| String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pr(number: u64, author: &str) -> Value {
        json!({
            "number": number, "title": "Add export", "body": "Why", "url": format!("https://github.com/acme/app/pull/{number}"),
            "isDraft": false, "createdAt": "2026-11-01T08:00:00Z", "updatedAt": format!("2026-11-0{number}T08:00:00Z"),
            "headRefName": "export", "baseRefName": "main", "headRefOid": "abc",
            "repository": {"nameWithOwner": "acme/app", "viewerDefaultMergeMethod": "SQUASH"},
            "author": {"login": author},
            "reviewDecision": "REVIEW_REQUIRED", "mergeable": "MERGEABLE", "mergeStateStatus": "BLOCKED",
            "latestReviews": {"nodes": [{"author": {"login": "sam"}, "state": "APPROVED"},
                                        {"author": {"login": "kim"}, "state": "COMMENTED"}]},
            "reviewRequests": {"nodes": [{"requestedReviewer": {"login": "me"}},
                                         {"requestedReviewer": {"slug": "core"}},
                                         {"requestedReviewer": {"login": "kim"}}]},
            "reviewThreads": {"nodes": [{"isResolved": true}, {"isResolved": false}]},
            "labels": {"nodes": [{"name": "bug", "color": "d73a4a"}]},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE"}}}]},
            "additions": 12, "deletions": 3
        })
    }

    #[test]
    fn parses_and_dedupes_lists() {
        let v = json!({"data": {
            "viewer": {"login": "me"},
            "requested": {"nodes": [pr(3, "alex"), {}]},
            "reviewed": {"nodes": [pr(3, "alex"), pr(5, "alex")]},
            "mine": {"nodes": [pr(4, "me")]}
        }});
        let d = parse(&v).unwrap();
        assert_eq!(d.review.len(), 2, "duplicate and non-PR result dropped");
        assert_eq!(d.review[0].iid, 5, "newest first");
        let r = &d.review[1];
        assert_eq!(r.reference(), "#3");
        assert_eq!(r.forge, Forge::GitHub);
        assert_eq!(r.viewer, "me");
        assert_eq!(r.project, "acme/app");
        assert_eq!(r.approved_by, vec!["sam"]);
        assert!(!r.approved);
        assert_eq!(r.pending_rules, vec!["required reviews"]);
        assert_eq!(r.review_of("me"), Some(ReviewState::Unreviewed));
        assert_eq!(
            r.review_of("kim"),
            Some(ReviewState::Unreviewed),
            "asked again"
        );
        assert_eq!(r.review_of("@core"), Some(ReviewState::Unreviewed));
        assert_eq!(r.threads_total, 2);
        assert_eq!(r.threads_open, 1);
        assert_eq!(r.pipeline, Pipeline::Failed);
        assert_eq!(r.merge_state, MergeState::Blocked);
        assert_eq!(r.labels[0].color, "#d73a4a");
        assert_eq!(r.merge_method, "squash");
        assert_eq!(d.mine[0].iid, 4);
    }

    #[test]
    fn activity_has_comments_reviews_and_threads() {
        let v = json!({"data": {"repository": {"pullRequest": {
            "comments": {"nodes": [{"body": "Looks good", "createdAt": "2026-11-01T08:00:00Z", "author": {"login": "sam"}}]},
            "reviews": {"nodes": [
                {"body": "", "state": "APPROVED", "submittedAt": "2026-11-03T08:00:00Z", "author": {"login": "sam"}},
                {"body": "", "state": "COMMENTED", "submittedAt": "2026-11-02T08:00:00Z", "author": {"login": "kim"}}
            ]},
            "reviewThreads": {"nodes": [{"id": "PRRT_1", "isResolved": false, "path": "src/a.rs", "line": null, "originalLine": 7,
                "comments": {"nodes": [{"body": "Why?", "createdAt": "2026-11-02T09:00:00Z", "author": {"login": "kim"}}]}}]}
        }}}});
        let a = parse_activity(&v).unwrap();
        assert_eq!(a.len(), 3);
        assert!(a[0].notes[0].system);
        assert_eq!(a[0].notes[0].body, "approved this pull request");
        assert_eq!(a[1].id, "PRRT_1");
        assert!(a[1].resolvable);
        assert_eq!(a[1].place.as_deref(), Some("src/a.rs:7"));
        assert!(!a[2].resolvable);
    }

    #[test]
    fn errors_are_reported() {
        let e = parse(&json!({"errors": [{"message": "boom"}]})).unwrap_err();
        assert!(e.contains("boom"));
    }
}
