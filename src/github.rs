//! GitHub: open, merged and closed pull requests, their review threads,
//! approve, merge and changes, through the `gh` CLI.

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

/// The fields the list shows of a pull request, for every query that loads
/// pull requests.
macro_rules! pr_fragment {
    () => {
        r#"
fragment pr on PullRequest {
  number title body url isDraft state createdAt updatedAt headRefName baseRefName headRefOid isCrossRepository
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
"#
    };
}

/// `KEY` is replaced per search; the search itself goes in as `$q`.
const QUERY: &str = concat!(
    r#"
query($q: String!) {
  viewer { login }
  KEY: search(query: $q, type: ISSUE, first: 30) { nodes { ...pr } }
}
"#,
    pr_fragment!()
);

/// What one search asks GitHub: the state, one of [`SEARCHES`]' terms and,
/// for merged and closed ones of one repository, that repository. Merged and
/// closed ones come newest updated first, since GitHub's own "best match"
/// would pick arbitrary old ones among hundreds.
pub fn search_terms(status: Status, terms: &str, repo: Option<&str>) -> String {
    let state = match status {
        Status::Open => "is:open",
        Status::Merged => "is:merged sort:updated-desc",
        Status::Closed => "is:closed is:unmerged sort:updated-desc",
    };
    let mut search = format!("{state} is:pr archived:false {terms}");
    if let Some(repo) = repo {
        search.push_str(&format!(" repo:{repo}"));
    }
    search
}

/// Whether `repo` is a plain `owner/name`, so it cannot add search terms of
/// its own.
pub fn plain_repo(repo: &str) -> bool {
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    matches!(repo.split_once('/'), Some((owner, name)) if ok(owner) && ok(name))
}

/// GitHub's word for the state in `pullRequests(states: …)`.
fn pr_states(status: Status) -> &'static str {
    match status {
        Status::Open => "OPEN",
        Status::Merged => "MERGED",
        // GitHub's CLOSED is closed without merging.
        Status::Closed => "CLOSED",
    }
}

/// How many pull requests "All" loads, newest first: as many as one search
/// above, since GitHub works out mergeability for each.
pub const ALL_LIMIT: usize = 30;

/// "All" in one repository: its pull requests in one state (`STATES`),
/// newest first, with the number in that state in all.
const REPO_ALL: &str = concat!(
    r#"
query($owner: String!, $name: String!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    nameWithOwner
    pullRequests(states: STATES, first: 30, orderBy: {field: UPDATED_AT, direction: DESC}) { totalCount nodes { ...pr } }
  }
}
"#,
    pr_fragment!()
);

/// "All" in your repositories, first step: the ones you own, collaborate on
/// or that belong to an organization you are a member of, most recently
/// pushed first, with the ids and update times of their newest pull
/// requests in one state (`STATES`) but none of their details.
const MEMBER_REPOS: &str = r#"
query {
  viewer {
    repositories(first: 100, affiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER], ownerAffiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER], isArchived: false, orderBy: {field: PUSHED_AT, direction: DESC}) {
      nodes {
        nameWithOwner
        pullRequests(states: STATES, first: 30, orderBy: {field: UPDATED_AT, direction: DESC}) { totalCount nodes { id updatedAt } }
      }
    }
  }
}
"#;

/// What the second step of "All" in your repositories loads.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberPick {
    /// The newest [`ALL_LIMIT`] open pull requests.
    pub ids: Vec<String>,
    /// Open pull requests in all those repositories.
    pub total: u64,
    /// Every repository of the answer (`owner/name`).
    pub repos: Vec<String>,
}

/// The ids of the newest `limit` open pull requests in the first step's
/// answer, and how many are open in all.
/// GitHub answers with errors next to the data the token may see, for
/// example for an organization whose single sign-on the gh login is not
/// authorized for; those repositories are then simply left out.
pub fn pick_members(value: &Value, limit: usize) -> Result<MemberPick, String> {
    if value.pointer("/data/viewer/repositories/nodes").is_none() {
        if let Some(msg) = cli::graphql_errors(value) {
            return Err(format!("GitHub: {msg}"));
        }
    }
    let mut total = 0;
    let mut found: Vec<(i64, String)> = Vec::new();
    let mut repos = Vec::new();
    for r in value
        .pointer("/data/viewer/repositories/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if r.is_null() {
            continue;
        }
        repos.push(str_of(&r["nameWithOwner"]));
        total += r["pullRequests"]["totalCount"].as_u64().unwrap_or(0);
        for n in r["pullRequests"]["nodes"].as_array().into_iter().flatten() {
            let id = str_of(&n["id"]);
            if !id.is_empty() {
                let at = n["updatedAt"].as_str().and_then(time::parse_iso);
                found.push((at.unwrap_or(0), id));
            }
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found.truncate(limit);
    Ok(MemberPick {
        ids: found.into_iter().map(|(_, id)| id).collect(),
        total,
        repos,
    })
}

/// The second step: these pull requests in full. The ids come from GitHub's
/// own answer and go in as JSON strings, which are valid GraphQL strings.
pub fn nodes_query(ids: &[String]) -> String {
    let list = ids
        .iter()
        .map(|id| serde_json::to_string(id).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "query {{ viewer {{ login }} nodes(ids: [{list}]) {{ ...pr }} }}\n{}",
        pr_fragment!()
    )
}

/// "All": every pull request in `status` of `repo` (`owner/name`), or of
/// your repositories when it is `None`, newest first, at most
/// [`ALL_LIMIT`]. `Ok(None)` when gh is not logged in.
pub fn fetch_all_of(status: Status, repo: Option<&str>) -> Result<Option<AllPart>, String> {
    let fetched = match repo {
        Some(repo) => fetch_repo_all(status, repo),
        None => fetch_member_all(status),
    };
    match fetched {
        Ok(part) => Ok(Some(part)),
        Err(Failure::NotLoggedIn) => Ok(None),
        Err(Failure::Other(e)) => Err(e),
    }
}

fn fetch_repo_all(status: Status, repo: &str) -> Result<AllPart, Failure> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| Failure::Other("not a GitHub repository".into()))?;
    let query = REPO_ALL.replace("STATES", pr_states(status));
    let value = graphql(&query, &[("owner", owner), ("name", name)])?;
    Ok(parse_repo_all(&value)?)
}

/// Two steps: which of your repositories have the newest open pull
/// requests, then those pull requests in full.
fn fetch_member_all(status: Status) -> Result<AllPart, Failure> {
    let query = MEMBER_REPOS.replace("STATES", pr_states(status));
    let pick = pick_members(&graphql(&query, &[])?, ALL_LIMIT)?;
    let mut part = if pick.ids.is_empty() {
        AllPart::default()
    } else {
        parse_nodes(&graphql(&nodes_query(&pick.ids), &[])?, pick.total)?
    };
    part.projects = pick.repos.iter().map(|r| format!("{HOST}/{r}")).collect();
    Ok(part)
}

fn viewer_of(value: &Value) -> Result<String, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitHub: {msg}"));
    }
    let viewer = str_of(&value["data"]["viewer"]["login"]);
    if viewer.is_empty() {
        return Err("GitHub did not return a user: run gh auth login".into());
    }
    Ok(viewer)
}

/// Newest first, each pull request once, at most [`ALL_LIMIT`].
fn newest(nodes: &[Value], viewer: &str, total: u64) -> AllPart {
    let mut items: Vec<MergeRequest> = Vec::new();
    for n in nodes {
        if let Some(pr) = parse_pr(n, viewer) {
            if !items.iter().any(|p| p.key() == pr.key()) {
                items.push(pr);
            }
        }
    }
    items.sort_by_key(|m| std::cmp::Reverse(m.updated));
    items.truncate(ALL_LIMIT);
    let total = total.max(items.len() as u64);
    AllPart {
        items,
        total,
        projects: Vec::new(),
    }
}

pub fn parse_repo_all(value: &Value) -> Result<AllPart, String> {
    let viewer = viewer_of(value)?;
    let list = &value["data"]["repository"]["pullRequests"];
    if list.is_null() {
        return Err(
            "GitHub did not return the repository: deleted, renamed or not visible to you".into(),
        );
    }
    let nodes = list["nodes"].as_array().cloned().unwrap_or_default();
    let mut part = newest(&nodes, &viewer, list["totalCount"].as_u64().unwrap_or(0));
    part.projects = vec![format!(
        "{HOST}/{}",
        str_of(&value["data"]["repository"]["nameWithOwner"])
    )];
    Ok(part)
}

/// The second step's answer, with the total from the first.
/// A pull request that is gone or hidden since the first step comes back
/// as an error next to the others; it is left out.
pub fn parse_nodes(value: &Value, total: u64) -> Result<AllPart, String> {
    let viewer = if value["data"]["nodes"].is_array() {
        let viewer = str_of(&value["data"]["viewer"]["login"]);
        if viewer.is_empty() {
            return Err("GitHub did not return a user: run gh auth login".into());
        }
        viewer
    } else {
        viewer_of(value)?
    };
    let nodes = value["data"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(newest(&nodes, &viewer, total))
}

/// Whether gh is installed at all; without it GitHub is left out quietly.
pub fn available() -> bool {
    cli::find("gh").is_some()
}

/// Your pull requests and those you review in `status`, of `repo` only when
/// it is given. `Ok(None)` when gh is not logged in: then GitHub is simply
/// not used.
pub fn fetch(status: Status, repo: Option<&str>) -> Result<Option<Data>, String> {
    if let Some(repo) = repo.filter(|r| !plain_repo(r)) {
        return Err(format!("{repo} is not a GitHub repository name"));
    }
    let answers: Vec<Result<Value, Failure>> = std::thread::scope(|scope| {
        let runs: Vec<_> = SEARCHES
            .iter()
            .map(|(key, terms)| {
                let search = search_terms(status, terms, repo);
                scope.spawn(move || graphql(&QUERY.replace("KEY", key), &[("q", &search)]))
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

impl From<String> for Failure {
    fn from(e: String) -> Failure {
        Failure::Other(e)
    }
}

impl From<Failure> for String {
    fn from(f: Failure) -> String {
        match f {
            Failure::NotLoggedIn => cli::login_hint("gh", HOST),
            Failure::Other(e) => e,
        }
    }
}

/// Runs a GraphQL query through gh. Variables starting with `$int:` are sent
/// as numbers, so text someone typed never goes in here (see [`reply`]).
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
    answer(&out)
}

/// GitHub's answer in gh's output: its JSON decides when there is one, gh's
/// error output only without it.
fn answer(out: &cli::Output) -> Result<Value, Failure> {
    // GitHub's own errors come as JSON, also when gh exits with an error.
    if let Ok(value) = serde_json::from_slice::<Value>(&out.stdout) {
        if !out.success && cli::refused(&value) {
            return Err(Failure::NotLoggedIn);
        }
        // A REST error answer (`{"message": …}`, such as a rate limit) has
        // neither GraphQL data nor errors: it is the error, in GitHub's words.
        if !out.success && value.get("data").is_none() && value.get("errors").is_none() {
            return Err(Failure::Other(cli::api_error("gh", HOST, "GitHub", out)));
        }
        return Ok(value);
    }
    if !out.success && cli::not_logged_in(&out.stderr) {
        return Err(Failure::NotLoggedIn);
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
        of: None,
        all: None,
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
        // Unknown counts as a fork: its branch is then fetched through the
        // pull request's own ref, which is right either way.
        from_fork: n["isCrossRepository"].as_bool().unwrap_or(true),
        head_sha: str_of(&n["headRefOid"]),
        target_branch: str_of(&n["baseRefName"]),
        created: n["createdAt"].as_str().and_then(time::parse_iso),
        updated: n["updatedAt"].as_str().and_then(time::parse_iso),
        merge_state,
        status: match n["state"].as_str() {
            Some("MERGED") => Status::Merged,
            Some("CLOSED") => Status::Closed,
            _ => Status::Open,
        },
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
      reviewThreads(last: 100) {
        nodes {
          id isResolved path line originalLine
          head: comments(first: 1) { nodes { id body createdAt author { login } } }
          tail: comments(last: 49) { totalCount nodes { id body createdAt author { login } } }
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
        hidden: 0,
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
        let end = |part: &str| -> Vec<(String, Note)> {
            t[part]["nodes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| (str_of(&c["id"]), note(c, "createdAt")))
                        .collect()
                })
                .unwrap_or_default()
        };
        let total = t["tail"]["totalCount"].as_u64().unwrap_or(0) as usize;
        let (notes, hidden) = thread_notes(end("head"), end("tail"), total);
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
            hidden,
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

const REPLY: &str = "mutation($thread: ID!, $body: String!) { addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { comment { state } } }";

/// The arguments of `gh api` that send a request whose body (JSON) comes on
/// standard input: the text never goes into an argument, where `-F` would
/// read `@…` as a file and a long text would meet the length limit of one
/// argument.
fn input_args(endpoint: &str, method: &str) -> Vec<String> {
    [
        "api",
        "--hostname",
        HOST,
        "--method",
        method,
        endpoint,
        "--header",
        "Content-Type: application/json",
        "--input",
        "-",
    ]
    .map(String::from)
    .to_vec()
}

/// The GraphQL request that replies `body` to the review thread `thread`.
fn reply_body(thread: &str, body: &str) -> Vec<u8> {
    serde_json::json!({ "query": REPLY, "variables": { "thread": thread, "body": body } })
        .to_string()
        .into_bytes()
}

/// Replies to a review thread. `Ok(true)` when GitHub kept the reply in a
/// review of yours that is not submitted yet, so nobody sees it until then.
pub fn reply(thread: &str, body: &str) -> Result<bool, String> {
    let mut cmd = cli::command("gh");
    cmd.args(input_args("graphql", "POST"));
    let out = cli::run_input("gh", cmd, HOST, reply_body(thread, body))?;
    let value = answer(&out)?;
    if let Some(msg) = cli::graphql_errors(&value) {
        return Err(format!("GitHub: {msg}"));
    }
    let state = value
        .pointer("/data/addPullRequestReviewThreadReply/comment/state")
        .and_then(Value::as_str)
        .ok_or("GitHub did not confirm the reply")?;
    Ok(state == "PENDING")
}

/// The REST endpoint of a pull request's conversation comments.
fn comments_endpoint(project: &str, iid: u64) -> String {
    format!("repos/{project}/issues/{iid}/comments")
}

/// Posts a comment on the pull request, in its conversation.
pub fn comment(mr: &MergeRequest, body: &str) -> Result<(), String> {
    let mut cmd = cli::command("gh");
    cmd.args(input_args(&comments_endpoint(&mr.project, mr.iid), "POST"));
    let data = serde_json::json!({ "body": body }).to_string().into_bytes();
    let out = cli::run_input("gh", cmd, HOST, data)?;
    if out.success {
        Ok(())
    } else {
        Err(cli::api_error("gh", HOST, "GitHub", &out))
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

/// Marks the pull request as draft, or as ready for review.
pub fn set_draft(mr: &MergeRequest, draft: bool) -> Result<(), String> {
    let args: &[&str] = if draft {
        &["ready", "--undo"]
    } else {
        &["ready"]
    };
    pr_command(mr, args).map(|_| ())
}

/// Approves the pull request at the head the list showed (`commit_id`), so
/// the review belongs to the code that was on screen; `gh pr review` has no
/// way to name the commit.
pub fn approve(mr: &MergeRequest) -> Result<(), String> {
    let mut cmd = cli::command("gh");
    cmd.args(approve_args(&mr.project, mr.iid, &mr.head_sha));
    let out = cli::run("gh", cmd, HOST)?;
    if out.success {
        Ok(())
    } else {
        Err(cli::explain_failure("gh", HOST, &out.stderr))
    }
}

fn approve_args(project: &str, iid: u64, head_sha: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "api",
        "--hostname",
        HOST,
        "--method",
        "POST",
        &format!("repos/{project}/pulls/{iid}/reviews"),
        "-f",
        "event=APPROVE",
    ]
    .map(String::from)
    .to_vec();
    if !head_sha.is_empty() {
        args.extend(["-f".to_string(), format!("commit_id={head_sha}")]);
    }
    args
}

/// The REST request that submits a review requesting changes with
/// `reason`, on the head the list showed (`head_sha`), so the review
/// belongs to the commit that was on screen.
fn request_changes_body(head_sha: &str, reason: &str) -> Vec<u8> {
    let mut review = serde_json::json!({ "event": "REQUEST_CHANGES", "body": reason });
    if !head_sha.is_empty() {
        review["commit_id"] = head_sha.into();
    }
    review.to_string().into_bytes()
}

/// Submits a review that requests changes, with `reason` as its text.
pub fn request_changes(mr: &MergeRequest, reason: &str) -> Result<(), String> {
    let endpoint = format!("repos/{}/pulls/{}/reviews", mr.project, mr.iid);
    let mut cmd = cli::command("gh");
    cmd.args(input_args(&endpoint, "POST"));
    let data = request_changes_body(&mr.head_sha, reason);
    let out = cli::run_input("gh", cmd, HOST, data)?;
    if out.success {
        Ok(())
    } else {
        Err(cli::api_error("gh", HOST, "GitHub", &out))
    }
}

/// The pull request's head and the check suites of its newest commit,
/// loaded when a retry is confirmed: the list loads only the rollup state.
const RETRY_HEAD: &str = r#"
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      headRefOid
      commits(last: 1) {
        nodes {
          commit {
            checkSuites(first: 100) {
              nodes { conclusion repository { nameWithOwner } workflowRun { databaseId } }
            }
          }
        }
      }
    }
  }
}
"#;

/// The failed GitHub Actions runs of the newest commit, as `(repository,
/// run id)`, from [`RETRY_HEAD`]'s answer, only while the head is still
/// `shown_head`. Other apps' checks and commit statuses can't be started
/// again with the user's login, so they are left out. `r` is `#2903`.
fn failed_runs(
    value: &Value,
    shown_head: &str,
    project: &str,
    r: &str,
) -> Result<Vec<(String, u64)>, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitHub: {msg}"));
    }
    let pr = &value["data"]["repository"]["pullRequest"];
    if pr.is_null() {
        return Err(format!("GitHub did not return {r}: refresh first (r)"));
    }
    if !shown_head.is_empty() && pr["headRefOid"].as_str() != Some(shown_head) {
        return Err(format!(
            "{r} changed since the list loaded: refresh first (r)"
        ));
    }
    let mut runs = Vec::new();
    let suites = pr
        .pointer("/commits/nodes/0/commit/checkSuites/nodes")
        .and_then(Value::as_array);
    for suite in suites.into_iter().flatten() {
        let failed = suite["conclusion"]
            .as_str()
            .is_some_and(|c| matches!(c, "FAILURE" | "TIMED_OUT" | "CANCELLED"));
        let Some(id) = suite["workflowRun"]["databaseId"].as_u64() else {
            continue;
        };
        if !failed {
            continue;
        }
        // The run lives where its suite does (the base repository, also
        // for a fork's pull request); a name GitHub gave that is no plain
        // `owner/name` never goes into the path.
        let repo = suite["repository"]["nameWithOwner"]
            .as_str()
            .filter(|n| plain_repo(n))
            .unwrap_or(project);
        runs.push((repo.to_string(), id));
    }
    if runs.is_empty() {
        return Err(format!(
            "No failed GitHub Actions run of {r} to start again: its other failed checks can't be started again from the list"
        ));
    }
    Ok(runs)
}

/// The arguments of `gh api` that start the failed jobs of one workflow
/// run again (REST "Re-run failed jobs from a workflow run").
fn rerun_args(repo: &str, run: u64) -> Vec<String> {
    [
        "api",
        "--hostname",
        HOST,
        "--method",
        "POST",
        &format!("repos/{repo}/actions/runs/{run}/rerun-failed-jobs"),
    ]
    .map(String::from)
    .to_vec()
}

/// What the list says after starting the failed runs of `r` again, from one
/// answer per run. When some started, it is a success (the list refreshes)
/// that names how many and why the others did not.
fn rerun_outcome(r: &str, results: Vec<Result<(), String>>) -> Result<String, String> {
    let total = results.len();
    let started = results.iter().filter(|r| r.is_ok()).count();
    // Each reason once: several runs are often refused for the same one.
    let mut why: Vec<String> = Vec::new();
    for e in results.into_iter().filter_map(Result::err) {
        if !why.contains(&e) {
            why.push(e);
        }
    }
    let why = why.join(" · ");
    match started {
        _ if started == total => Ok(format!("Failed checks of {r} started again")),
        0 => Err(why),
        _ => Ok(format!(
            "Started {started} of {total} failed runs of {r} again; the others did not: {why}"
        )),
    }
}

/// Starts the failed jobs of every failed GitHub Actions run of the pull
/// request's newest commit again, only if its head is still the one the
/// list showed.
pub fn retry_checks(mr: &MergeRequest) -> Result<String, String> {
    let r = mr.reference();
    let (owner, name) = mr
        .project
        .split_once('/')
        .ok_or("not a GitHub repository")?;
    let number = format!("$int:{}", mr.iid);
    let head = graphql(
        RETRY_HEAD,
        &[("owner", owner), ("name", name), ("number", &number)],
    )?;
    let runs = failed_runs(&head, &mr.head_sha, &mr.project, &r)?;
    let results = runs
        .iter()
        .map(|(repo, run)| {
            let mut cmd = cli::command("gh");
            cmd.args(rerun_args(repo, *run));
            let out = cli::run("gh", cmd, HOST)?;
            if out.success {
                Ok(())
            } else {
                Err(cli::api_error("gh", HOST, "GitHub", &out))
            }
        })
        .collect();
    rerun_outcome(&r, results)
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

    #[test]
    fn approval_is_pinned_to_the_shown_head() {
        assert_eq!(
            approve_args("acme/app", 7, "abc123"),
            [
                "api",
                "--hostname",
                "github.com",
                "--method",
                "POST",
                "repos/acme/app/pulls/7/reviews",
                "-f",
                "event=APPROVE",
                "-f",
                "commit_id=abc123",
            ]
        );
        assert_eq!(approve_args("acme/app", 7, "").len(), 8);
    }

    #[test]
    fn a_reply_names_its_thread_and_a_comment_its_pull_request() {
        assert_eq!(
            input_args("graphql", "POST"),
            [
                "api",
                "--hostname",
                "github.com",
                "--method",
                "POST",
                "graphql",
                "--header",
                "Content-Type: application/json",
                "--input",
                "-",
            ]
        );
        let sent: Value = serde_json::from_slice(&reply_body("PRRT_kwDOabc", "Done.")).unwrap();
        assert_eq!(
            sent,
            json!({"query": REPLY, "variables": {"thread": "PRRT_kwDOabc", "body": "Done."}})
        );
        assert_eq!(
            comments_endpoint("acme/app", 2903),
            "repos/acme/app/issues/2903/comments"
        );
    }

    #[test]
    fn a_change_request_is_a_review_on_the_shown_head() {
        let sent: Value =
            serde_json::from_slice(&request_changes_body("abc123", "Rename it.")).unwrap();
        assert_eq!(
            sent,
            json!({"event": "REQUEST_CHANGES", "body": "Rename it.", "commit_id": "abc123"})
        );
        let sent: Value = serde_json::from_slice(&request_changes_body("", "Rename it.")).unwrap();
        assert_eq!(
            sent,
            json!({"event": "REQUEST_CHANGES", "body": "Rename it."})
        );
    }

    #[test]
    fn a_change_request_reason_is_sent_as_typed() {
        for reason in [
            "/merge later",
            "\"quotes\", {owner} and $int:5",
            "Bitte ändern: Grösse 👍\nzweite Zeile",
            &"long ".repeat(200),
        ] {
            let sent: Value = serde_json::from_slice(&request_changes_body("abc", reason)).unwrap();
            assert_eq!(sent["body"], reason);
        }
    }

    #[test]
    fn reply_text_is_sent_as_typed() {
        for body in [
            "@alex please check",
            "$int:5",
            "$int:@/etc/passwd",
            "two\nlines",
            "\"quotes\" and {owner}",
        ] {
            let sent: Value = serde_json::from_slice(&reply_body("PRRT_x", body)).unwrap();
            assert_eq!(sent["variables"]["body"], body);
        }
    }

    fn pr(number: u64, author: &str) -> Value {
        json!({
            "number": number, "title": "Add export", "body": "Why", "url": format!("https://github.com/acme/app/pull/{number}"),
            "isDraft": false, "createdAt": "2026-11-01T08:00:00Z", "updatedAt": format!("2026-11-0{number}T08:00:00Z"),
            "headRefName": "export", "baseRefName": "main", "headRefOid": "abc",
            "isCrossRepository": number == 5,
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
        assert!(!r.from_fork);
        assert!(d.review[0].from_fork, "#5 comes from a fork");
    }

    #[test]
    fn reads_whether_a_pull_request_is_open_merged_or_closed() {
        let with = |state: &str| {
            let mut n = pr(3, "alex");
            n["state"] = json!(state);
            n
        };
        let v = json!({"data": {
            "viewer": {"login": "me"},
            "mine": {"nodes": [with("MERGED"), with("CLOSED"), with("OPEN"), pr(4, "me")]}
        }});
        let d = parse(&v).unwrap();
        // Each pull request once: the first answer counts.
        assert_eq!(d.mine[1].status, Status::Merged);
        assert_eq!(d.mine[0].status, Status::Open, "no state: open");
        let v = json!({"data": {"viewer": {"login": "me"}, "mine": {"nodes": [with("CLOSED")]}}});
        assert_eq!(parse(&v).unwrap().mine[0].status, Status::Closed);
    }

    #[test]
    fn searches_ask_for_the_state_newest_first_and_the_repository() {
        // Open ones exactly as before.
        assert_eq!(
            search_terms(Status::Open, "author:@me", None),
            "is:open is:pr archived:false author:@me"
        );
        assert_eq!(
            search_terms(Status::Merged, "review-requested:@me", None),
            "is:merged sort:updated-desc is:pr archived:false review-requested:@me"
        );
        assert_eq!(
            search_terms(Status::Closed, "author:@me", Some("acme/app")),
            "is:closed is:unmerged sort:updated-desc is:pr archived:false author:@me repo:acme/app"
        );
        // The search is a variable, never part of the query text; at most 30.
        assert!(QUERY.contains("search(query: $q, type: ISSUE, first: 30)"));
        assert!(!QUERY.contains("is:open"));
    }

    #[test]
    fn only_plain_repository_names_go_into_a_search() {
        assert!(plain_repo("acme/app"));
        assert!(plain_repo("acme-co/app.web_2"));
        for bad in [
            "acme",
            "acme/app is:open",
            "acme/app/x",
            "/app",
            "acme/",
            "acme/\"x",
        ] {
            assert!(!plain_repo(bad), "{bad}");
        }
        assert!(fetch(Status::Merged, Some("acme/app author:sam")).is_err());
    }

    #[test]
    fn all_asks_for_the_state() {
        assert!(REPO_ALL
            .replace("STATES", pr_states(Status::Open))
            .contains("states: OPEN"));
        assert!(MEMBER_REPOS
            .replace("STATES", pr_states(Status::Merged))
            .contains("pullRequests(states: MERGED, first: 30"));
        assert_eq!(pr_states(Status::Closed), "CLOSED");
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
                "head": {"nodes": [{"id": "PRRC_1", "body": "Why?", "createdAt": "2026-11-02T09:00:00Z", "author": {"login": "kim"}}]},
                "tail": {"totalCount": 1, "nodes": [{"id": "PRRC_1", "body": "Why?", "createdAt": "2026-11-02T09:00:00Z", "author": {"login": "kim"}}]}}]}
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

    /// A comment of a review thread as `ACTIVITY` loads it.
    fn thread_comment(id: u32, author: &str, at: &str) -> Value {
        json!({"id": format!("PRRC_{id}"), "body": format!("Comment {id}"), "createdAt": at, "author": {"login": author}})
    }

    #[test]
    fn activity_loads_the_newest_hundred_threads_with_both_ends() {
        assert!(ACTIVITY.contains("reviewThreads(last: 100)"), "{ACTIVITY}");
        assert!(ACTIVITY.contains("head: comments(first: 1)"), "{ACTIVITY}");
        assert!(
            ACTIVITY.contains("tail: comments(last: 49) { totalCount"),
            "{ACTIVITY}"
        );
    }

    #[test]
    fn review_threads_show_both_ends_once() {
        let at = |h: u32, m: u32| format!("2026-11-02T{h:02}:{m:02}:00Z");
        // 60 comments, one a minute: the tail has comments 12 to 60.
        let tail: Vec<Value> = (12..=60)
            .map(|i| thread_comment(i, "sam", &at(8 + (i - 1) / 60, (i - 1) % 60)))
            .collect();
        let short = [
            thread_comment(100, "kim", &at(8, 30)),
            thread_comment(101, "me", &at(8, 31)),
        ];
        let v = json!({"data": {"repository": {"pullRequest": {
            "comments": {"nodes": []}, "reviews": {"nodes": []},
            "reviewThreads": {"nodes": [
                {"id": "PRRT_short", "isResolved": false, "path": "src/b.rs", "line": 3, "originalLine": 3,
                 "head": {"nodes": [short[0].clone()]},
                 "tail": {"totalCount": 2, "nodes": short}},
                {"id": "PRRT_long", "isResolved": true, "path": "src/a.rs", "line": null, "originalLine": 7,
                 "head": {"nodes": [thread_comment(1, "alex", &at(8, 0))]},
                 "tail": {"totalCount": 60, "nodes": tail}}
            ]}
        }}}});
        let a = parse_activity(&v).unwrap();
        // The long thread sorts by its real newest reply (08:59).
        assert_eq!(a[0].id, "PRRT_long");
        let long = &a[0];
        assert_eq!(long.notes.len(), 50);
        assert_eq!(long.notes[0].body, "Comment 1");
        assert_eq!(long.notes[1].body, "Comment 12");
        assert_eq!(long.notes[49].body, "Comment 60");
        assert_eq!(long.hidden, 10, "comments 2 to 11");
        assert_eq!(long.place.as_deref(), Some("src/a.rs:7"));
        assert!(long.resolvable && long.resolved);
        // The short one: each comment once, oldest first.
        let bodies: Vec<&str> = a[1].notes.iter().map(|n| n.body.as_str()).collect();
        assert_eq!(bodies, ["Comment 100", "Comment 101"]);
        assert_eq!(a[1].hidden, 0);
    }

    #[test]
    fn repo_all_has_everyones_once_with_the_total() {
        let v = json!({"data": {"viewer": {"login": "me"}, "repository": {"nameWithOwner": "acme/app", "pullRequests": {
            "totalCount": 54,
            "nodes": [pr(3, "alex"), pr(4, "me"), pr(5, "sam"), pr(3, "alex")]
        }}}});
        let part = parse_repo_all(&v).unwrap();
        let numbers: Vec<u64> = part.items.iter().map(|m| m.iid).collect();
        assert_eq!(numbers, [5, 4, 3], "newest first, each once");
        assert_eq!(part.total, 54);
        assert!(part.items.iter().all(|m| m.viewer == "me"));
        assert_eq!(part.projects, ["github.com/acme/app"]);
    }

    #[test]
    fn repo_all_without_the_repository_says_so() {
        let v = json!({"data": {"viewer": {"login": "me"}, "repository": null}});
        let e = parse_repo_all(&v).unwrap_err();
        assert!(e.contains("did not return the repository"), "{e}");
    }

    #[test]
    fn member_pick_takes_the_newest_ids() {
        let v = json!({"data": {"viewer": {"repositories": {"nodes": [
            {"nameWithOwner": "acme/app", "pullRequests": {"totalCount": 40, "nodes": [
                {"id": "PR_a", "updatedAt": "2026-11-08T09:00:00Z"},
                {"id": "PR_b", "updatedAt": "2026-11-01T09:00:00Z"}]}},
            {"nameWithOwner": "acme/docs", "pullRequests": {"totalCount": 0, "nodes": []}},
            {"nameWithOwner": "acme/lib", "pullRequests": {"totalCount": 1, "nodes": [
                {"id": "PR_c", "updatedAt": "2026-11-07T09:00:00Z"}]}}
        ]}}}});
        let pick = pick_members(&v, 2).unwrap();
        assert_eq!(pick.ids, ["PR_a", "PR_c"]);
        assert_eq!(pick.total, 41);
        assert_eq!(pick.repos, ["acme/app", "acme/docs", "acme/lib"]);
    }

    #[test]
    fn errors_next_to_data_leave_only_those_out() {
        // An organization behind single sign-on: its repository is null.
        let v = json!({"data": {"viewer": {"repositories": {"nodes": [
            {"nameWithOwner": "acme/app", "pullRequests": {"totalCount": 1, "nodes": [
                {"id": "PR_a", "updatedAt": "2026-11-08T09:00:00Z"}]}},
            null
        ]}}}, "errors": [{"type": "FORBIDDEN", "message": "SAML enforcement"}]});
        let pick = pick_members(&v, 30).unwrap();
        assert_eq!(pick.ids, ["PR_a"]);
        assert_eq!(pick.repos, ["acme/app"]);
        // A pull request deleted between the two steps.
        let v = json!({"data": {"viewer": {"login": "me"}, "nodes": [pr(3, "alex"), null]},
                       "errors": [{"type": "NOT_FOUND", "message": "gone"}]});
        assert_eq!(parse_nodes(&v, 2).unwrap().items.len(), 1);
        // Without data the error is the answer.
        let e = pick_members(&json!({"errors": [{"message": "boom"}]}), 30).unwrap_err();
        assert!(e.contains("boom"));
        let e = parse_nodes(&json!({"errors": [{"message": "boom"}]}), 0).unwrap_err();
        assert!(e.contains("boom"));
    }

    #[test]
    fn nodes_query_names_ids_as_strings() {
        let q = nodes_query(&["PR_a".to_string(), "PR_\"b".to_string()]);
        assert!(q.contains(r#"nodes(ids: ["PR_a", "PR_\"b"])"#), "{q}");
        assert!(q.contains("fragment pr on PullRequest"));
    }

    #[test]
    fn nodes_keep_the_first_steps_total() {
        let v = json!({"data": {"viewer": {"login": "me"}, "nodes": [pr(3, "alex"), {}]}});
        let part = parse_nodes(&v, 41).unwrap();
        assert_eq!(part.items.len(), 1);
        assert_eq!(part.total, 41);
    }

    fn output(success: bool, stdout: &str, stderr: &str) -> cli::Output {
        cli::Output {
            success,
            code: Some(if success { 0 } else { 1 }),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn a_refused_login_leaves_github_out() {
        // What gh 2.x writes without a login, and with a token GitHub
        // refuses (HTTP 401).
        let logged_out = output(
            false,
            "",
            "To get started with GitHub CLI, please run:  gh auth login\nAlternatively, populate the GH_TOKEN environment variable with a GitHub API authentication token.",
        );
        assert!(matches!(answer(&logged_out), Err(Failure::NotLoggedIn)));
        let refused = output(
            false,
            r#"{"message":"Bad credentials","documentation_url":"https://docs.github.com/rest","status":"401"}"#,
            "gh: Bad credentials (HTTP 401)",
        );
        assert!(matches!(answer(&refused), Err(Failure::NotLoggedIn)));
        assert_eq!(
            error_of(&refused),
            "gh is not logged in to github.com: run gh auth login"
        );
    }

    fn error_of(out: &cli::Output) -> String {
        match answer(out) {
            Err(f) => String::from(f),
            Ok(_) => panic!("an error"),
        }
    }

    #[test]
    fn errors_that_mention_401_stay_errors() {
        // A GraphQL error while logged in: GitHub's own answer, shown.
        let complex = output(
            false,
            r#"{"errors":[{"message":"Query has complexity of 401, which exceeds max complexity of 400"}]}"#,
            "gh: Query has complexity of 401, which exceeds max complexity of 400",
        );
        let Ok(value) = answer(&complex) else {
            panic!("GitHub's answer");
        };
        assert_eq!(
            parse(&value).unwrap_err(),
            "GitHub: Query has complexity of 401, which exceeds max complexity of 400"
        );
        // A REST error answer is the error, also when it mentions 401.
        let rest = output(
            false,
            r#"{"message":"Rate limit of 401 requests exceeded","status":"403"}"#,
            "gh: Rate limit of 401 requests exceeded (HTTP 403)",
        );
        assert_eq!(
            error_of(&rest),
            "GitHub: Rate limit of 401 requests exceeded"
        );
        // Without JSON, the tool's error output is the error.
        let unauthorized = output(false, "", "gh: Unauthorized to view repository acme/app401");
        assert_eq!(
            error_of(&unauthorized),
            "gh failed: gh: Unauthorized to view repository acme/app401"
        );
        let offline = output(
            false,
            "",
            "Post \"https://api.github.com/graphql\": dial tcp: lookup api.github.com: no such host",
        );
        assert_eq!(
            error_of(&offline),
            "Cannot reach github.com: check the connection"
        );
    }

    #[test]
    fn errors_are_reported() {
        let e = parse(&json!({"errors": [{"message": "boom"}]})).unwrap_err();
        assert!(e.contains("boom"));
    }

    /// The answer of the retry query for a head `sha` with `suites`.
    fn retry_head(sha: &str, suites: Value) -> Value {
        json!({"data": {"repository": {"pullRequest": {
            "headRefOid": sha,
            "commits": {"nodes": [{"commit": {"checkSuites": {"nodes": suites}}}]}
        }}}})
    }

    fn suite(conclusion: Option<&str>, run: Option<u64>) -> Value {
        json!({
            "conclusion": conclusion,
            "repository": {"nameWithOwner": "acme/app"},
            "workflowRun": run.map(|id| json!({"databaseId": id})),
        })
    }

    #[test]
    fn a_retry_picks_the_failed_actions_runs() {
        let suites = json!([
            suite(Some("FAILURE"), Some(11)),
            suite(Some("SUCCESS"), Some(12)),
            suite(Some("SKIPPED"), Some(13)),
            suite(Some("TIMED_OUT"), Some(14)),
            suite(Some("CANCELLED"), Some(15)),
            // Another app's check: no workflow run.
            suite(Some("FAILURE"), None),
            // Still running.
            suite(None, Some(16)),
            // Waiting for a maintainer's approval, or a broken workflow
            // file: a re-run can't start them.
            suite(Some("ACTION_REQUIRED"), Some(17)),
            suite(Some("STARTUP_FAILURE"), Some(18)),
        ]);
        let runs = failed_runs(&retry_head("abc123", suites), "abc123", "acme/app", "#2903");
        let ids: Vec<u64> = runs.unwrap().into_iter().map(|(_, id)| id).collect();
        assert_eq!(ids, [11, 14, 15]);
    }

    #[test]
    fn a_run_is_started_where_its_suite_lives() {
        let mut s = suite(Some("FAILURE"), Some(11));
        s["repository"]["nameWithOwner"] = json!("acme/base");
        let runs = failed_runs(&retry_head("abc", json!([s])), "abc", "acme/app", "#1");
        assert_eq!(runs, Ok(vec![("acme/base".into(), 11)]));
        // A name that is no plain `owner/name` never goes into the path.
        let mut s = suite(Some("FAILURE"), Some(11));
        s["repository"]["nameWithOwner"] = json!("acme/app/../../user");
        let runs = failed_runs(&retry_head("abc", json!([s])), "abc", "acme/app", "#1");
        assert_eq!(runs, Ok(vec![("acme/app".into(), 11)]));
    }

    #[test]
    fn other_apps_checks_are_not_started() {
        let only_other = json!([
            suite(Some("FAILURE"), None),
            suite(Some("SUCCESS"), Some(3))
        ]);
        let expected = Err(
            "No failed GitHub Actions run of #2903 to start again: its other failed checks can't be started again from the list"
                .to_string(),
        );
        assert_eq!(
            failed_runs(&retry_head("abc", only_other), "abc", "acme/app", "#2903"),
            expected
        );
        // A commit status of an outside CI service brings no suite at all.
        assert_eq!(
            failed_runs(&retry_head("abc", json!([])), "abc", "acme/app", "#2903"),
            expected
        );
    }

    #[test]
    fn checks_of_a_pushed_head_are_not_started() {
        let suites = json!([suite(Some("FAILURE"), Some(11))]);
        assert_eq!(
            failed_runs(&retry_head("def456", suites), "abc123", "acme/app", "#2903"),
            Err("#2903 changed since the list loaded: refresh first (r)".into())
        );
        // Without a head from the list there is nothing to compare, like
        // approving and merging.
        let suites = json!([suite(Some("FAILURE"), Some(11))]);
        assert!(failed_runs(&retry_head("def456", suites), "", "acme/app", "#2903").is_ok());
        let refused = json!({"errors": [{"message": "Could not resolve to a Repository"}]});
        assert!(failed_runs(&refused, "abc", "acme/app", "#2903")
            .unwrap_err()
            .starts_with("GitHub: Could not resolve"));
    }

    #[test]
    fn a_rerun_asks_for_the_failed_jobs_only() {
        assert_eq!(
            rerun_args("acme/app", 4711),
            [
                "api",
                "--hostname",
                "github.com",
                "--method",
                "POST",
                "repos/acme/app/actions/runs/4711/rerun-failed-jobs",
            ]
        );
    }

    #[test]
    fn a_rerun_says_how_many_started() {
        assert_eq!(
            rerun_outcome("#2903", vec![Ok(()), Ok(())]),
            Ok("Failed checks of #2903 started again".into())
        );
        assert_eq!(
            rerun_outcome(
                "#2903",
                vec![
                    Ok(()),
                    Err("GitHub: This workflow run cannot be retried".into()),
                    Err("GitHub: This workflow run cannot be retried".into()),
                ]
            ),
            Ok("Started 1 of 3 failed runs of #2903 again; the others did not: GitHub: This workflow run cannot be retried".into())
        );
        assert_eq!(
            rerun_outcome(
                "#2903",
                vec![Err("GitHub: Resource not accessible by integration".into())]
            ),
            Err("GitHub: Resource not accessible by integration".into())
        );
    }
}
