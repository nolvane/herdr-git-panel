//! GitLab: open, merged and closed merge requests, their threads, approve,
//! merge and changes, through the `glab` CLI.

use crate::cli;
use crate::model::*;
use crate::time;
use serde_json::Value;

/// The fields the list shows of a merge request, for every query that loads
/// merge requests.
macro_rules! mr_fragment {
    () => {
        r#"
fragment mr on MergeRequest {
  iid title description webUrl draft state createdAt updatedAt sourceBranch targetBranch diffHeadSha
  sourceProjectId targetProjectId
  project { fullPath }
  author { username }
  approved approvalsRequired approvalsLeft
  approvedBy { nodes { username } }
  approvalState { rules { name type approved approvalsRequired } }
  reviewers { nodes { username mergeRequestInteraction { reviewState } } }
  detailedMergeStatus
  resolvableDiscussionsCount resolvedDiscussionsCount
  labels { nodes { title color } }
  headPipeline { status }
  diffStatsSummary { additions deletions }
}
"#
    };
}

/// GitLab's word for the state, as in `mergeRequests(state: …)`.
fn state_word(status: Status) -> &'static str {
    match status {
        Status::Open => "opened",
        Status::Merged => "merged",
        Status::Closed => "closed",
    }
}

/// One list per request: both in one query exceed GitLab's complexity limit
/// (250). `list` is the list's field name. Merged and closed ones come
/// newest updated first and, when `scoped`, of the project in `$project`
/// only; open ones as always.
pub fn list_query(list: &str, status: Status, scoped: bool) -> String {
    let (vars, args) = match (status, scoped) {
        (Status::Open, _) => ("", "state: opened".to_string()),
        (s, false) => ("", format!("state: {}, sort: UPDATED_DESC", state_word(s))),
        (s, true) => (
            "($project: String!)",
            format!(
                "state: {}, sort: UPDATED_DESC, projectPath: $project",
                state_word(s)
            ),
        ),
    };
    format!(
        "query{vars} {{\n  currentUser {{\n    username\n    {list}({args}, first: 100) {{ nodes {{ ...mr }} }}\n  }}\n}}\n{}",
        mr_fragment!()
    )
}

/// How many merge requests "All" loads, newest first: GitLab's largest page.
pub const ALL_LIMIT: usize = 100;

/// "All" in one project: its merge requests in one state (`STATE`), newest
/// first, with the number in that state in all. Complexity about 140 of 250.
const PROJECT_ALL: &str = concat!(
    r#"
query($project: ID!) {
  currentUser { username }
  project(fullPath: $project) {
    fullPath
    mergeRequests(state: STATE, first: 100, sort: UPDATED_DESC) { count nodes { ...mr } }
  }
}
"#,
    mr_fragment!()
);

/// "All" in your projects, first step: the projects you are a member of,
/// most recently active first, with when their merge requests in one state
/// (`STATE`) were updated, but none of their details (complexity about 30).
const MEMBER_PROJECTS: &str = r#"
query {
  currentUser { username }
  projects(membership: true, archived: EXCLUDE, withMergeRequestsEnabled: true, sort: "latest_activity_desc", first: 100) {
    nodes {
      fullPath
      mergeRequests(state: STATE, first: 100, sort: UPDATED_DESC) { count nodes { updatedAt } }
    }
  }
}
"#;

/// Projects per request in the second step: each one adds about 1.4 to the
/// complexity, 50 come to about 205 of 250.
const PROJECTS_PER_REQUEST: usize = 50;

/// What the second step of "All" in your projects loads.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberPick {
    /// The projects that hold the newest [`ALL_LIMIT`] open merge requests.
    pub paths: Vec<String>,
    /// Load only merge requests updated after this (ISO 8601), when there
    /// are more than the limit: one second before the oldest one picked, so
    /// ties at the cut are not lost.
    pub after: Option<String>,
    /// Open merge requests in all those projects.
    pub total: u64,
    /// Every project of the answer, also those without open merge requests.
    pub members: Vec<String>,
}

/// Picks the newest `limit` open merge requests from the first step's
/// answer.
pub fn pick_members(value: &Value, limit: usize) -> Result<MemberPick, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitLab: {msg}"));
    }
    let mut total = 0;
    let mut times: Vec<(i64, String)> = Vec::new();
    let mut members = Vec::new();
    for p in value
        .pointer("/data/projects/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let path = str_of(&p["fullPath"]);
        members.push(path.clone());
        total += num_of(&p["mergeRequests"]["count"]);
        for n in p["mergeRequests"]["nodes"].as_array().into_iter().flatten() {
            if let Some(t) = n["updatedAt"].as_str().and_then(time::parse_iso) {
                times.push((t, path.clone()));
            }
        }
    }
    times.sort_by_key(|a| std::cmp::Reverse(a.0));
    let cut = total > limit as u64 || times.len() > limit;
    // The update time of the oldest one picked; projects with one as new
    // (a tie at the cut) are asked too.
    let oldest = times
        .get(limit.min(times.len()).saturating_sub(1))
        .map(|t| t.0);
    let after = oldest.filter(|_| cut).map(|t| time::iso(t - 1));
    let mut paths: Vec<String> = Vec::new();
    for (_, path) in times.into_iter().take_while(|t| Some(t.0) >= oldest) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(MemberPick {
        paths,
        after,
        total,
        members,
    })
}

/// The second step: the merge requests of these projects, from `after` on.
/// Paths and time come from GitLab's own answer and go in as JSON strings,
/// which are valid GraphQL strings.
pub fn member_query(paths: &[String], after: Option<&str>, status: Status) -> String {
    let list = paths
        .iter()
        .map(|p| serde_json::to_string(p).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(", ");
    let after = after
        .map(|t| {
            format!(
                ", updatedAfter: {}",
                serde_json::to_string(t).unwrap_or_default()
            )
        })
        .unwrap_or_default();
    format!(
        "query {{ currentUser {{ username }} projects(fullPaths: [{list}], first: {}) {{ nodes {{ mergeRequests(state: {state}{after}, first: 100, sort: UPDATED_DESC) {{ nodes {{ ...mr }} }} }} }} }}\n{}",
        paths.len(),
        mr_fragment!(),
        state = state_word(status),
    )
}

/// "All": every merge request in `status` of `project`, or of the projects
/// you are a member of when it is `None`, newest first, at most
/// [`ALL_LIMIT`]. `Ok(None)` when glab is not logged in.
pub fn fetch_all_of(
    host: &str,
    status: Status,
    project: Option<&str>,
) -> Result<Option<AllPart>, String> {
    if let Some(path) = project {
        let query = PROJECT_ALL.replace("STATE", state_word(status));
        let Some(value) = user_graphql(host, &query, &[("project", path)])? else {
            return Ok(None);
        };
        return parse_project_all(&value, host).map(Some);
    }
    let query = MEMBER_PROJECTS.replace("STATE", state_word(status));
    let Some(members) = user_graphql(host, &query, &[])? else {
        return Ok(None);
    };
    let pick = pick_members(&members, ALL_LIMIT)?;
    let answers: Vec<Result<Option<Value>, String>> = std::thread::scope(|scope| {
        let runs: Vec<_> = pick
            .paths
            .chunks(PROJECTS_PER_REQUEST)
            .map(|chunk| {
                let query = member_query(chunk, pick.after.as_deref(), status);
                scope.spawn(move || user_graphql(host, &query, &[]))
            })
            .collect();
        runs.into_iter()
            .map(|r| r.join().unwrap_or_else(|_| Err("glab crashed".into())))
            .collect()
    });
    let Some(answers) = all_answers(answers)? else {
        return Ok(None);
    };
    let mut items = Vec::new();
    for answer in answers {
        items.extend(parse_members(&answer, host)?);
    }
    let mut part = newest(items, pick.total, ALL_LIMIT);
    part.projects = pick.members.iter().map(|p| format!("{host}/{p}")).collect();
    Ok(Some(part))
}

/// Newest first, each merge request once, at most `limit`.
fn newest(mut items: Vec<MergeRequest>, total: u64, limit: usize) -> AllPart {
    items.sort_by_key(|m| std::cmp::Reverse(m.updated));
    let mut seen = std::collections::HashSet::new();
    items.retain(|m| seen.insert(m.key()));
    items.truncate(limit);
    let total = total.max(items.len() as u64);
    AllPart {
        items,
        total,
        projects: Vec::new(),
    }
}

fn viewer_of(value: &Value) -> Result<String, String> {
    let viewer = str_of(&value["data"]["currentUser"]["username"]);
    if viewer.is_empty() {
        return Err("GitLab did not return a user: run glab auth login".into());
    }
    Ok(viewer)
}

pub fn parse_project_all(value: &Value, host: &str) -> Result<AllPart, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitLab: {msg}"));
    }
    let viewer = viewer_of(value)?;
    let list = &value["data"]["project"]["mergeRequests"];
    if list.is_null() {
        return Err(
            "GitLab did not return the project: deleted, renamed or not visible to you".into(),
        );
    }
    let items = list["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| parse_mr(n, host, &viewer))
        .collect();
    let mut part = newest(items, num_of(&list["count"]), ALL_LIMIT);
    part.projects = vec![format!(
        "{host}/{}",
        str_of(&value["data"]["project"]["fullPath"])
    )];
    Ok(part)
}

/// The merge requests of the second step's answer.
pub fn parse_members(value: &Value, host: &str) -> Result<Vec<MergeRequest>, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitLab: {msg}"));
    }
    let viewer = viewer_of(value)?;
    Ok(value
        .pointer("/data/projects/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|p| p["mergeRequests"]["nodes"].as_array().into_iter().flatten())
        .filter_map(|n| parse_mr(n, host, &viewer))
        .collect())
}

/// Your merge requests and those you review in `status`, of `project` only
/// when it is given (merged and closed ones). `Ok(None)` when glab is not
/// logged in: then GitLab is simply not used, like GitHub without gh's
/// login.
pub fn fetch(host: &str, status: Status, project: Option<&str>) -> Result<Option<Data>, String> {
    // Both lists at the same time.
    let (review, mine) = std::thread::scope(|scope| {
        let mine = scope.spawn(|| query(host, "authoredMergeRequests", status, project));
        let review = query(host, "reviewRequestedMergeRequests", status, project);
        (
            review,
            mine.join().unwrap_or_else(|_| Err("glab crashed".into())),
        )
    });
    let Some((mut merged, mine)) = both_lists(review, mine)? else {
        return Ok(None);
    };
    if merged.get("errors").is_none() && mine.get("errors").is_some() {
        merged = mine;
    } else if let Some(list) = mine
        .pointer("/data/currentUser/authoredMergeRequests")
        .cloned()
    {
        if let Some(user) = merged
            .pointer_mut("/data/currentUser")
            .and_then(Value::as_object_mut)
        {
            user.insert("authoredMergeRequests".into(), list);
        }
    }
    parse(&merged, host).map(Some)
}

/// Both answers of [`fetch`], `None` when either was not logged in. An error
/// wins: it may be why the other one looked logged out.
fn both_lists(
    review: Result<Option<Value>, String>,
    mine: Result<Option<Value>, String>,
) -> Result<Option<(Value, Value)>, String> {
    Ok(review?.zip(mine?))
}

/// The second step's answers, `None` when one was not logged in (logged
/// out between the steps). An error wins, as in [`both_lists`].
fn all_answers(answers: Vec<Result<Option<Value>, String>>) -> Result<Option<Vec<Value>>, String> {
    let answers: Vec<Option<Value>> = answers.into_iter().collect::<Result<_, _>>()?;
    Ok(answers.into_iter().collect())
}

/// Runs one list query through glab and returns GitLab's JSON answer,
/// `None` when glab is not logged in.
fn query(
    host: &str,
    list: &str,
    status: Status,
    project: Option<&str>,
) -> Result<Option<Value>, String> {
    // Only a query that declares `$project` may get it.
    let project = project.filter(|_| status != Status::Open);
    let vars: Vec<(&str, &str)> = project.map(|p| ("project", p)).into_iter().collect();
    user_graphql(host, &list_query(list, status, project.is_some()), &vars)
}

/// Why a GitLab request failed.
enum Failure {
    /// glab has no token for the host, or GitLab refused it.
    NotLoggedIn,
    Other(String),
}

/// Runs a GraphQL query through glab, with variables, and returns GitLab's
/// JSON answer.
fn request(host: &str, query: &str, vars: &[(&str, &str)]) -> Result<Value, Failure> {
    let mut cmd = cli::command("glab");
    cmd.args(["api", "graphql", "--hostname", host, "-f"])
        .arg(format!("query={query}"));
    for (k, v) in vars {
        cmd.arg("-f").arg(format!("{k}={v}"));
    }
    let out = cli::run("glab", cmd, host).map_err(Failure::Other)?;
    answer(&out, host)
}

/// GitLab's answer in glab's output.
fn answer(out: &cli::Output, host: &str) -> Result<Value, Failure> {
    // GitLab's own errors come as JSON, also when glab exits with an error.
    if let Ok(value) = serde_json::from_slice::<Value>(&out.stdout) {
        // A refusal comes without data; one error next to data is not one.
        let no_data = value.get("data").is_none_or(Value::is_null);
        if refused(&value) && (!out.success || no_data) {
            return Err(Failure::NotLoggedIn);
        }
        return Ok(value);
    }
    if !out.success && refused_stderr(&out.stderr) {
        return Err(Failure::NotLoggedIn);
    }
    if !out.success {
        return Err(Failure::Other(cli::explain_failure(
            "glab",
            host,
            &out.stderr,
        )));
    }
    Err(Failure::Other("unreadable answer from GitLab".into()))
}

/// Whether GitLab's JSON answer refuses the token: without one, or with
/// one it no longer takes, it answers 401 with exactly "Invalid token"
/// (GraphQL), `invalid_token` (an expired OAuth token) or "401
/// Unauthorized" (REST). Whole messages only, so an error that merely
/// mentions 401 or a token (a query's complexity, an integration) stays an
/// error.
fn refused(value: &Value) -> bool {
    let is = |v: Option<&Value>, word: &str| {
        v.and_then(Value::as_str)
            .is_some_and(|m| m.trim().eq_ignore_ascii_case(word))
    };
    let errors = value.get("errors").and_then(Value::as_array);
    errors
        .into_iter()
        .flatten()
        .any(|e| is(e.get("message"), "Invalid token"))
        || is(value.get("error"), "invalid_token")
        || is(value.get("message"), "401 Unauthorized")
}

/// Whether glab's error output, without a JSON answer, says it is not
/// logged in. Stricter than [`cli::not_logged_in`]: a bare "401" may be
/// part of a host name, and here it would hide GitLab without a word.
fn refused_stderr(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    [
        "(http 401)",
        "401 unauthorized",
        "invalid token",
        "invalid_token",
        "not logged in",
        "not authenticated",
        "auth login",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// [`request`] for one merge request the user acts on: not being logged in
/// is an error that says so.
fn graphql(host: &str, query: &str, vars: &[(&str, &str)]) -> Result<Value, String> {
    request(host, query, vars).map_err(|f| match f {
        Failure::NotLoggedIn => cli::login_hint("glab", host),
        Failure::Other(e) => e,
    })
}

/// [`request`] for the lists: `Ok(None)` when glab is not logged in to
/// `host`, which then leaves GitLab out quietly, like gh.
fn user_graphql(host: &str, query: &str, vars: &[(&str, &str)]) -> Result<Option<Value>, String> {
    match request(host, query, vars) {
        Ok(value) if anonymous(&value) => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(Failure::NotLoggedIn) => Ok(None),
        Err(Failure::Other(e)) => Err(e),
    }
}

/// Whether GitLab answered without a user and without an error: glab sent
/// no token, as for a host it was never logged in to. Every list query asks
/// for `currentUser`, so this can tell.
fn anonymous(value: &Value) -> bool {
    cli::graphql_errors(value).is_none()
        && value
            .pointer("/data/currentUser")
            .is_some_and(Value::is_null)
}

/// Runs a `glab mr …` command on the merge request and returns its output.
fn mr_command(mr: &MergeRequest, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = cli::command("glab");
    cmd.arg("mr")
        .args(&args[..1])
        .arg(mr.iid.to_string())
        .args(&args[1..])
        .arg("--repo")
        .arg(format!("https://{}/{}", mr.host, mr.project));
    let out = cli::run("glab", cmd, &mr.host)?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(cli::explain_failure("glab", &mr.host, &out.stderr))
    }
}

const ACTIVITY: &str = r#"
query($project: ID!, $iid: String!) {
  project(fullPath: $project) {
    mergeRequest(iid: $iid) {
      discussions(first: 100) {
        nodes {
          id resolvable resolved
          notes(first: 50) {
            nodes { body system createdAt author { username } position { filePath newLine oldLine } }
          }
        }
      }
    }
  }
}
"#;

/// The discussions of a merge request, newest activity first.
pub fn fetch_activity(mr: &MergeRequest) -> Result<Vec<Discussion>, String> {
    let iid = mr.iid.to_string();
    let value = graphql(
        &mr.host,
        ACTIVITY,
        &[("project", &mr.project), ("iid", &iid)],
    )?;
    parse_activity(&value)
}

pub fn parse_activity(value: &Value) -> Result<Vec<Discussion>, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitLab: {msg}"));
    }
    let nodes = value
        .pointer("/data/project/mergeRequest/discussions/nodes")
        .and_then(Value::as_array)
        .ok_or("GitLab returned no discussions")?;
    let mut list: Vec<Discussion> = nodes
        .iter()
        .map(|d| {
            let notes: Vec<Note> = d["notes"]["nodes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|n| Note {
                            author: str_of(&n["author"]["username"]),
                            body: str_of(&n["body"]),
                            created: n["createdAt"].as_str().and_then(time::parse_iso),
                            system: n["system"].as_bool().unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let first = &d["notes"]["nodes"][0]["position"];
            let place = first["filePath"].as_str().map(|path| {
                match first["newLine"]
                    .as_u64()
                    .or_else(|| first["oldLine"].as_u64())
                {
                    Some(line) => format!("{path}:{line}"),
                    None => path.to_string(),
                }
            });
            Discussion {
                id: str_of(&d["id"]),
                resolvable: d["resolvable"].as_bool().unwrap_or(false),
                resolved: d["resolved"].as_bool().unwrap_or(false),
                place,
                notes,
            }
        })
        .filter(|d| !d.notes.is_empty())
        .collect();
    // Newest activity first: by the last note of each discussion.
    list.sort_by_key(|d| std::cmp::Reverse(d.notes.iter().filter_map(|n| n.created).max()));
    Ok(list)
}

/// Marks a thread as resolved, or opens it again.
pub fn set_resolved(host: &str, discussion: &str, resolve: bool) -> Result<(), String> {
    let mutation = format!(
        "mutation($id: DiscussionID!) {{ discussionToggleResolve(input: {{id: $id, resolve: {resolve}}}) {{ errors }} }}"
    );
    let value = graphql(host, &mutation, &[("id", discussion)])?;
    if let Some(msg) = cli::graphql_errors(&value) {
        return Err(format!("GitLab: {msg}"));
    }
    let errors = value
        .pointer("/data/discussionToggleResolve/errors")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("GitLab: {errors}"))
    }
}

/// A project path as one segment of a REST path: `acme/app` → `acme%2Fapp`.
fn encode_path(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The arguments of `glab api` that post a note on merge request `iid`: a
/// reply to the discussion `thread` (GraphQL's global id, such as
/// `gid://gitlab/Discussion/<hex>`), or a comment on the merge request
/// itself. The text goes on standard input ([`note_body`]), never in an
/// argument, so glab reads nothing in it as a file or `:placeholder`.
fn note_args(
    host: &str,
    project: &str,
    iid: u64,
    thread: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut endpoint = format!("projects/{}/merge_requests/{iid}", encode_path(project));
    if let Some(thread) = thread {
        let hex = thread
            .strip_prefix("gid://gitlab/Discussion/")
            .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("GitLab: this is not a thread that takes replies")?;
        endpoint.push_str(&format!("/discussions/{hex}"));
    }
    endpoint.push_str("/notes");
    Ok([
        "api",
        "--hostname",
        host,
        "--method",
        "POST",
        &endpoint,
        "--input",
        "-",
    ]
    .map(String::from)
    .to_vec())
}

/// The request body of a note: the text as typed, as JSON.
fn note_body(body: &str) -> Vec<u8> {
    serde_json::json!({ "body": body }).to_string().into_bytes()
}

/// `text` with a backslash in front of every line that starts with `/`
/// (`\/merge later`). GitLab runs such lines of a note as quick actions;
/// escaped, it leaves them as text and Markdown shows `/merge later`.
fn escape_quick_actions(text: &str) -> String {
    let escaped: Vec<String> = text
        .split('\n')
        .map(|line| {
            if line.starts_with('/') {
                format!("\\{line}")
            } else {
                line.to_string()
            }
        })
        .collect();
    escaped.join("\n")
}

/// The request body of a reply or comment: the text as typed, as JSON,
/// with lines starting with `/` escaped so GitLab runs no command from
/// them.
fn text_note_body(body: &str) -> Vec<u8> {
    note_body(&escape_quick_actions(body))
}

/// Posts `body` as a reply to a thread (`Some`) or as a comment on the
/// merge request.
pub fn post_note(mr: &MergeRequest, thread: Option<&str>, body: &str) -> Result<(), String> {
    let mut cmd = cli::command("glab");
    cmd.args(note_args(&mr.host, &mr.project, mr.iid, thread)?);
    let out = cli::run_input("glab", cmd, &mr.host, text_note_body(body))?;
    if out.success {
        Ok(())
    } else {
        Err(cli::api_error("glab", &mr.host, "GitLab", &out))
    }
}

/// The quick action that submits your review as "requested changes".
const REQUEST_CHANGES: &str = "/submit_review requested_changes";

/// The note that requests changes with `reason`: the quick action first,
/// then the reason. GitLab runs every line that starts with `/` as a quick
/// action, so each such line of the reason gets a backslash in front
/// (`\/merge later`), which GitLab leaves as text and Markdown shows as
/// `/merge later`. The quick action comes first, so nothing in the reason
/// (an open code block, an HTML block) can hide it; GitLab takes it out
/// of the posted note.
fn request_changes_note(reason: &str) -> String {
    format!("{REQUEST_CHANGES}\n\n{}", escape_quick_actions(reason))
}

/// Whether GitLab ran the quick action of a change request: it takes the
/// line out of the note it saves, so a saved note that still holds it
/// (a server before 17.2, or one with request changes off) means only the
/// comment was posted. `stdout` is GitLab's answer to the note.
fn took_change_request(stdout: &[u8]) -> bool {
    serde_json::from_slice::<Value>(stdout)
        .ok()
        .and_then(|v| v["body"].as_str().map(str::to_string))
        .is_none_or(|body| !body.lines().any(|l| l.trim() == REQUEST_CHANGES))
}

/// Requests changes with `reason`: a comment on the merge request that
/// also submits your review as "requested changes".
pub fn request_changes(mr: &MergeRequest, reason: &str) -> Result<(), String> {
    let mut cmd = cli::command("glab");
    cmd.args(note_args(&mr.host, &mr.project, mr.iid, None)?);
    let note = request_changes_note(reason);
    let out = cli::run_input("glab", cmd, &mr.host, note_body(&note))?;
    if !out.success {
        return Err(cli::api_error("glab", &mr.host, "GitLab", &out));
    }
    if took_change_request(&out.stdout) {
        Ok(())
    } else {
        Err(format!(
            "GitLab posted the reason as a comment but did not request changes: {} needs GitLab 17.2 or newer with request changes on",
            mr.host
        ))
    }
}

/// Merges the merge request with its own settings (squash, delete branch),
/// only if its head is still the one the list showed.
pub fn merge(mr: &MergeRequest) -> Result<(), String> {
    let mut args = vec!["merge", "--yes", "--auto-merge=false"];
    if !mr.head_sha.is_empty() {
        args.extend(["--sha", mr.head_sha.as_str()]);
    }
    mr_command(mr, &args).map(|_| ())
}

/// Marks the merge request as draft, or as ready.
pub fn set_draft(mr: &MergeRequest, draft: bool) -> Result<(), String> {
    mr_command(mr, &["update", if draft { "--draft" } else { "--ready" }]).map(|_| ())
}

/// Approves the merge request, or takes your approval back (`revoke`).
/// An approval is pinned to the head the list showed, so a push after it
/// loaded makes GitLab refuse the approval instead of approving unseen code.
pub fn set_approval(mr: &MergeRequest, revoke: bool) -> Result<(), String> {
    mr_command(mr, &approval_args(revoke, &mr.head_sha)).map(|_| ())
}

fn approval_args(revoke: bool, head_sha: &str) -> Vec<&str> {
    if revoke {
        vec!["revoke"]
    } else if head_sha.is_empty() {
        vec!["approve"]
    } else {
        vec!["approve", "--sha", head_sha]
    }
}

/// The merge request's changes as a unified diff, without colours.
pub fn fetch_diff(mr: &MergeRequest) -> Result<String, String> {
    mr_command(mr, &["diff", "--color", "never", "--raw"])
        .map(|out| String::from_utf8_lossy(&out).into_owned())
}

pub fn parse(value: &Value, host: &str) -> Result<Data, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitLab: {msg}"));
    }
    let user = &value["data"]["currentUser"];
    if user.is_null() {
        return Err("GitLab did not return a user: run glab auth login".into());
    }
    let viewer = str_of(&user["username"]);
    let list = |key: &str| -> Vec<MergeRequest> {
        let mut mrs: Vec<MergeRequest> = user[key]["nodes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| parse_mr(n, host, &viewer))
                    .collect()
            })
            .unwrap_or_default();
        mrs.sort_by_key(|m| std::cmp::Reverse(m.updated));
        mrs
    };
    Ok(Data {
        review: list("reviewRequestedMergeRequests"),
        mine: list("authoredMergeRequests"),
        of: None,
        all: None,
    })
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn num_of(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// Whether the merge request's branch lives in another project (a fork).
/// An unknown source project counts as a fork: the branch is then fetched
/// through the merge request's own ref, which is right either way.
fn from_fork(n: &Value) -> bool {
    match (n["sourceProjectId"].as_u64(), n["targetProjectId"].as_u64()) {
        (Some(source), Some(target)) => source != target,
        _ => true,
    }
}

fn parse_mr(n: &Value, host: &str, viewer: &str) -> Option<MergeRequest> {
    let iid = n["iid"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| n["iid"].as_u64())?;
    let draft = n["draft"].as_bool().unwrap_or(false);
    let merge_state = match n["detailedMergeStatus"].as_str().unwrap_or("") {
        _ if draft => MergeState::Draft,
        "MERGEABLE" => MergeState::Ready,
        "DRAFT_STATUS" => MergeState::Draft,
        "CONFLICT" => MergeState::Conflict,
        "NEED_REBASE" => MergeState::NeedsRebase,
        "CHECKING" | "UNCHECKED" | "PREPARING" | "CI_STILL_RUNNING" => MergeState::Checking,
        _ => MergeState::Blocked,
    };
    let required = num_of(&n["approvalsRequired"]);
    let approver_names: Vec<String> = n["approvedBy"]["nodes"]
        .as_array()
        .map(|a| a.iter().map(|u| str_of(&u["username"])).collect())
        .unwrap_or_default();
    let approved_by = approver_names.len() as u64;
    let reviewers = n["reviewers"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| Reviewer {
                    username: str_of(&r["username"]),
                    state: match r["mergeRequestInteraction"]["reviewState"]
                        .as_str()
                        .unwrap_or("")
                    {
                        "APPROVED" => ReviewState::Approved,
                        "REQUESTED_CHANGES" => ReviewState::ChangesRequested,
                        "REVIEWED" => ReviewState::Reviewed,
                        "REVIEW_STARTED" => ReviewState::Started,
                        _ => ReviewState::Unreviewed,
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    let mut pending_rules: Vec<String> = Vec::new();
    // Each rule once: GitLab's own total counts code owner sections twice.
    let rules_required: u64 = n["approvalState"]["rules"]
        .as_array()
        .map(|a| a.iter().map(|r| num_of(&r["approvalsRequired"])).sum())
        .unwrap_or(0);
    for rule in n["approvalState"]["rules"].as_array().into_iter().flatten() {
        if rule["approved"].as_bool().unwrap_or(true) || num_of(&rule["approvalsRequired"]) == 0 {
            continue;
        }
        let name = match rule["type"].as_str().unwrap_or("") {
            // A code owner rule is named after its path pattern.
            "CODE_OWNER" => "code owners".to_string(),
            "ANY_APPROVER" => "any approver".to_string(),
            _ => str_of(&rule["name"]),
        };
        if !name.is_empty() && !pending_rules.contains(&name) {
            pending_rules.push(name);
        }
    }
    let total = num_of(&n["resolvableDiscussionsCount"]);
    let resolved = num_of(&n["resolvedDiscussionsCount"]);
    let pipeline = match n["headPipeline"]["status"].as_str() {
        None => Pipeline::None,
        Some("SUCCESS") => Pipeline::Success,
        Some("FAILED") => Pipeline::Failed,
        Some("CANCELED") | Some("CANCELING") | Some("SKIPPED") => Pipeline::Canceled,
        Some("MANUAL") | Some("SCHEDULED") => Pipeline::Manual,
        Some(_) => Pipeline::Running,
    };
    let labels = n["labels"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| Label {
                    title: str_of(&l["title"]),
                    color: str_of(&l["color"]),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(MergeRequest {
        forge: Forge::GitLab,
        host: host.to_string(),
        viewer: viewer.to_string(),
        iid,
        title: str_of(&n["title"]),
        description: str_of(&n["description"]),
        url: str_of(&n["webUrl"]),
        project: str_of(&n["project"]["fullPath"]),
        author: str_of(&n["author"]["username"]),
        source_branch: str_of(&n["sourceBranch"]),
        from_fork: from_fork(n),
        head_sha: str_of(&n["diffHeadSha"]),
        target_branch: str_of(&n["targetBranch"]),
        created: n["createdAt"].as_str().and_then(time::parse_iso),
        updated: n["updatedAt"].as_str().and_then(time::parse_iso),
        merge_state,
        status: match n["state"].as_str() {
            Some("merged") => Status::Merged,
            Some("closed") => Status::Closed,
            // `opened`, and `locked` while GitLab merges it.
            _ => Status::Open,
        },
        approvals_given: approved_by,
        approvals_required: Some(if rules_required > 0 {
            rules_required
        } else {
            required
        }),
        approved_by: approver_names,
        approved: n["approved"].as_bool().unwrap_or(false) && (required > 0 || approved_by > 0),
        pending_rules,
        reviewers,
        threads_total: total,
        threads_open: total.saturating_sub(resolved),
        labels,
        pipeline,
        additions: n["diffStatsSummary"]["additions"].as_u64(),
        deletions: n["diffStatsSummary"]["deletions"].as_u64(),
        merge_method: String::new(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn approval_is_pinned_to_the_shown_head() {
        assert_eq!(
            approval_args(false, "abc123"),
            ["approve", "--sha", "abc123"]
        );
        assert_eq!(approval_args(false, ""), ["approve"]);
        assert_eq!(approval_args(true, "abc123"), ["revoke"]);
    }

    #[test]
    fn a_reply_goes_to_its_discussion_and_a_comment_to_the_merge_request() {
        let reply = note_args(
            "gitlab.com",
            "acme/web app",
            90,
            Some("gid://gitlab/Discussion/3f2a9c"),
        )
        .unwrap();
        assert_eq!(
            reply,
            [
                "api",
                "--hostname",
                "gitlab.com",
                "--method",
                "POST",
                "projects/acme%2Fweb%20app/merge_requests/90/discussions/3f2a9c/notes",
                "--input",
                "-",
            ]
        );
        let comment = note_args("git.acme.test", "acme/app", 7, None).unwrap();
        assert_eq!(comment[2], "git.acme.test");
        assert_eq!(comment[5], "projects/acme%2Fapp/merge_requests/7/notes");
    }

    #[test]
    fn note_body_wraps_the_text_as_json() {
        // The plain JSON wrapper; replies and comments also escape `/` lines.
        for body in [
            "@alex please check",
            "@/etc/passwd",
            "line one\nline two",
            "see :branch and :id",
            "[a,b]",
            "\"quotes\" and \\ backslashes",
        ] {
            let sent: Value = serde_json::from_slice(&note_body(body)).unwrap();
            assert_eq!(sent, json!({ "body": body }));
        }
    }

    #[test]
    fn a_reply_or_comment_runs_no_quick_action() {
        let sent: Value = serde_json::from_slice(&text_note_body(
            "/merge later\nok /approve here\n/approve\n /close",
        ))
        .unwrap();
        assert_eq!(
            sent["body"],
            "\\/merge later\nok /approve here\n\\/approve\n /close"
        );
        let body = sent["body"].as_str().unwrap();
        assert!(!body.lines().any(|l| l.starts_with('/')));
    }

    #[test]
    fn a_reply_or_comment_is_sent_as_typed_otherwise() {
        for body in [
            "@alex please check",
            "@/etc/passwd and a/b",
            "line one\nline two",
            "see :branch and :id",
            "\"quotes\" and \\ backslashes",
            "Bitte ändern: Grösse, Übersicht 👍",
        ] {
            let sent: Value = serde_json::from_slice(&text_note_body(body)).unwrap();
            assert_eq!(sent, json!({ "body": body }));
        }
    }

    #[test]
    fn a_change_request_submits_the_review_before_the_reason() {
        assert_eq!(
            request_changes_note("Please rename it."),
            "/submit_review requested_changes\n\nPlease rename it."
        );
        // The quick action leads, also when the reason opens a block that
        // GitLab would read commands in as text.
        let note = request_changes_note("```\nunclosed");
        assert!(note.starts_with("/submit_review requested_changes\n"));
    }

    #[test]
    fn a_saved_note_that_still_holds_the_quick_action_requested_nothing() {
        // As gitlab.com answered on 2026-11-09: the quick action taken out.
        assert!(took_change_request(
            br#"{"id":1,"body":"Please rename it.","commands_changes":{}}"#
        ));
        // A server that did not run it keeps the line in the note.
        assert!(!took_change_request(
            br#"{"id":1,"body":"/submit_review requested_changes\n\nPlease rename it."}"#
        ));
        // The escaped reason itself does not count.
        assert!(took_change_request(
            br#"{"id":1,"body":"\\/submit_review requested_changes"}"#
        ));
        // An answer that is no note (only commands) counts as taken.
        assert!(took_change_request(br#"{"commands_changes":{}}"#));
    }

    #[test]
    fn a_reason_runs_no_quick_action() {
        let note = request_changes_note("/merge later\nok /approve here\n/approve\n /close");
        assert_eq!(
            note,
            "/submit_review requested_changes\n\n\\/merge later\nok /approve here\n\\/approve\n /close"
        );
        // Also inside a code block, where GitLab would not run it: there
        // the backslash shows (recorded in docs/decisions.md).
        assert!(request_changes_note("```\n/usr/bin/env\n```").contains("\n\\/usr/bin/env\n"));
        // Only the quick action of the list starts a line with `/`.
        let commands: Vec<&str> = note.lines().filter(|l| l.starts_with('/')).collect();
        assert_eq!(commands, ["/submit_review requested_changes"]);
    }

    #[test]
    fn a_reason_is_sent_as_typed_otherwise() {
        for reason in [
            "\"quotes\" and 'single' and \\ backslashes",
            "Bitte ändern: Grösse, Übersicht 👍",
            "@alex see :branch and @/etc/passwd",
            &"long ".repeat(200),
        ] {
            let note = request_changes_note(reason);
            let sent: Value = serde_json::from_slice(&note_body(&note)).unwrap();
            assert_eq!(
                sent["body"].as_str().unwrap(),
                format!("/submit_review requested_changes\n\n{reason}")
            );
        }
    }

    #[test]
    fn only_gitlab_discussion_ids_take_replies() {
        for thread in [
            "",
            "PRRT_kwDOabc",
            "gid://gitlab/Discussion/",
            "gid://gitlab/Discussion/../../projects",
            "gid://gitlab/Note/12",
        ] {
            assert!(
                note_args("gitlab.com", "acme/app", 7, Some(thread)).is_err(),
                "{thread}"
            );
        }
    }

    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({"data": {"currentUser": {
            "username": "me",
            "reviewRequestedMergeRequests": {"nodes": [{
                "iid": "42", "title": "Add export", "webUrl": "https://gitlab.com/acme/app/-/merge_requests/42",
                "draft": false, "updatedAt": "2026-11-08T08:00:00Z",
                "sourceBranch": "abc-1-export", "targetBranch": "main",
                "sourceProjectId": 11, "targetProjectId": 11,
                "project": {"fullPath": "acme/app"}, "author": {"username": "alex"},
                "approved": false, "approvalsRequired": 1, "approvalsLeft": 1,
                "approvedBy": {"nodes": []},
                "approvalState": {"rules": [
                    {"name": "*, /docs/", "type": "CODE_OWNER", "approved": false, "approvalsRequired": 1},
                    {"name": "Builders", "type": "REGULAR", "approved": false, "approvalsRequired": 1},
                    {"name": "Optional", "type": "REGULAR", "approved": false, "approvalsRequired": 0}
                ]},
                "reviewers": {"nodes": [{"username": "me", "mergeRequestInteraction": {"reviewState": "UNREVIEWED"}}]},
                "detailedMergeStatus": "NOT_APPROVED",
                "resolvableDiscussionsCount": 3, "resolvedDiscussionsCount": 1,
                "labels": {"nodes": [{"title": "backend", "color": "#428BCA"}]},
                "headPipeline": {"status": "RUNNING"},
                "diffStatsSummary": {"additions": 120, "deletions": 8}
            }]},
            "authoredMergeRequests": {"nodes": [
                {"iid": "7", "title": "Old", "webUrl": "u7", "draft": true, "updatedAt": "2026-11-01T08:00:00Z",
                 "project": {"fullPath": "acme/app"}, "author": {"username": "me"},
                 "approved": true, "approvalsRequired": 0, "approvalsLeft": 0, "approvedBy": {"nodes": []},
                 "reviewers": {"nodes": []}, "detailedMergeStatus": "DRAFT_STATUS",
                 "resolvableDiscussionsCount": 0, "resolvedDiscussionsCount": 0,
                 "labels": {"nodes": []}, "headPipeline": null, "diffStatsSummary": null},
                {"iid": "9", "title": "New", "webUrl": "u9", "draft": false, "updatedAt": "2026-11-07T08:00:00Z",
                 "sourceProjectId": 12, "targetProjectId": 11,
                 "project": {"fullPath": "acme/lib"}, "author": {"username": "me"},
                 "approved": true, "approvalsRequired": 2, "approvalsLeft": 0, "approvedBy": {"nodes": [{"username": "alex"}]},
                 "reviewers": {"nodes": [{"username": "alex", "mergeRequestInteraction": {"reviewState": "APPROVED"}}]},
                 "detailedMergeStatus": "MERGEABLE",
                 "resolvableDiscussionsCount": 2, "resolvedDiscussionsCount": 2,
                 "labels": {"nodes": []}, "headPipeline": {"status": "SUCCESS"}, "diffStatsSummary": {"additions": 1, "deletions": 0}}
            ]}
        }}})
    }

    #[test]
    fn parses_both_lists() {
        let d = parse(&sample(), "gitlab.com").unwrap();
        assert_eq!(d.review[0].viewer, "me");
        assert_eq!(d.review[0].reference(), "!42");
        let r = &d.review[0];
        assert_eq!(r.iid, 42);
        assert_eq!(r.project_name(), "app");
        assert_eq!(r.threads_open, 2);
        assert_eq!(r.approvals_given, 0);
        assert_eq!(r.pipeline, Pipeline::Running);
        assert_eq!(r.merge_state, MergeState::Blocked);
        assert_eq!(r.review_of("me"), Some(ReviewState::Unreviewed));
        // Newest first.
        assert_eq!(d.mine[0].iid, 9);
        assert_eq!(d.mine[0].approvals_given, 1);
        assert_eq!(d.mine[0].approved_by, vec!["alex"]);
        assert!(d.mine[0].pending_rules.is_empty());
        assert_eq!(d.review[0].pending_rules, vec!["code owners", "Builders"]);
        assert_eq!(d.review[0].approvals_required, Some(2), "one per rule");
        assert_eq!(d.mine[0].merge_state, MergeState::Ready);
        assert!(d.mine[0].approved);
        assert_eq!(d.mine[1].merge_state, MergeState::Draft);
        assert!(!d.mine[1].approved, "no approval rule and nobody approved");
        assert_eq!(d.mine[1].pipeline, Pipeline::None);
        assert!(!d.review[0].from_fork, "same project");
        assert!(d.mine[0].from_fork, "from another project");
        assert!(d.mine[1].from_fork, "source unknown: the safe side");
    }

    #[test]
    fn activity_newest_first_with_place() {
        let v = json!({"data": {"project": {"mergeRequest": {"discussions": {"nodes": [
            {"resolvable": false, "resolved": false, "notes": {"nodes": [
                {"body": "approved this merge request", "system": true, "createdAt": "2026-11-01T08:00:00Z",
                 "author": {"username": "alex"}, "position": null}]}},
            {"id": "gid://gitlab/Discussion/abc", "resolvable": true, "resolved": false, "notes": {"nodes": [
                {"body": "Why?", "system": false, "createdAt": "2026-11-02T08:00:00Z",
                 "author": {"username": "sam"}, "position": {"filePath": "src/a.rs", "newLine": 12, "oldLine": null}},
                {"body": "Because.", "system": false, "createdAt": "2026-11-03T08:00:00Z",
                 "author": {"username": "me"}, "position": null}]}},
            {"resolvable": false, "resolved": false, "notes": {"nodes": []}}
        ]}}}}});
        let a = parse_activity(&v).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].place.as_deref(), Some("src/a.rs:12"));
        assert_eq!(a[0].id, "gid://gitlab/Discussion/abc");
        assert_eq!(a[0].notes.len(), 2);
        assert!(a[1].notes[0].system);
    }

    /// A merge request node of `project` by `author`, updated at `at`.
    fn node(iid: u64, project: &str, author: &str, reviewer: Option<&str>, at: &str) -> Value {
        let reviewers: Vec<Value> = reviewer
            .map(|r| json!({"username": r, "mergeRequestInteraction": {"reviewState": "UNREVIEWED"}}))
            .into_iter()
            .collect();
        json!({"iid": iid.to_string(), "title": format!("Change {iid}"), "webUrl": format!("https://gitlab.com/{project}/-/merge_requests/{iid}"),
               "draft": false, "updatedAt": at, "project": {"fullPath": project}, "author": {"username": author},
               "approved": false, "approvalsRequired": 0, "approvedBy": {"nodes": []},
               "reviewers": {"nodes": reviewers}, "detailedMergeStatus": "NOT_APPROVED",
               "resolvableDiscussionsCount": 0, "resolvedDiscussionsCount": 0,
               "labels": {"nodes": []}, "headPipeline": null, "diffStatsSummary": null})
    }

    #[test]
    fn reads_whether_a_merge_request_is_open_merged_or_closed() {
        let with = |iid, state: &str| {
            let mut n = node(iid, "acme/app", "me", None, "2026-11-08T07:00:00Z");
            n["state"] = json!(state);
            n
        };
        let v = json!({"data": {"currentUser": {"username": "me", "authoredMergeRequests": {"nodes": [
            with(1, "merged"), with(2, "closed"), with(3, "opened"), with(4, "locked"),
            node(5, "acme/app", "me", None, "2026-11-08T07:00:00Z")
        ]}}}});
        let d = parse(&v, "gitlab.com").unwrap();
        let by = |iid| d.mine.iter().find(|m| m.iid == iid).unwrap().status;
        assert_eq!(by(1), Status::Merged);
        assert_eq!(by(2), Status::Closed);
        assert_eq!(by(3), Status::Open);
        assert_eq!(by(4), Status::Open, "locked while GitLab merges it");
        assert_eq!(by(5), Status::Open);
    }

    #[test]
    fn list_queries_ask_for_the_state_newest_first_and_the_project() {
        let open = list_query("authoredMergeRequests", Status::Open, false);
        assert!(open.starts_with("query {"), "no variables");
        assert!(open.contains("authoredMergeRequests(state: opened, first: 100)"));
        // Open ones are never asked per project.
        assert_eq!(
            list_query("authoredMergeRequests", Status::Open, true),
            open
        );
        let merged = list_query("reviewRequestedMergeRequests", Status::Merged, false);
        assert!(merged.starts_with("query {"), "no unused variable");
        assert!(merged.contains(
            "reviewRequestedMergeRequests(state: merged, sort: UPDATED_DESC, first: 100)"
        ));
        let closed = list_query("authoredMergeRequests", Status::Closed, true);
        // GitLab's projectPath is a String, not an ID.
        assert!(closed.starts_with("query($project: String!) {"));
        assert!(closed.contains(
            "authoredMergeRequests(state: closed, sort: UPDATED_DESC, projectPath: $project, first: 100)"
        ));
        assert!(closed.contains("fragment mr on MergeRequest"));
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
    fn a_refused_token_is_not_logged_in() {
        // What glab 1.128 does on gitlab.com without a token, or with one
        // GitLab refuses (HTTP 401).
        let refused = output(
            false,
            r#"{"errors":[{"message":"Invalid token"}]}"#,
            "glab: Invalid token",
        );
        assert!(matches!(
            answer(&refused, "gitlab.com"),
            Err(Failure::NotLoggedIn)
        ));
        let expired = output(
            false,
            r#"{"error":"invalid_token","error_description":"Token is expired. You can either do re-authorization or token refresh."}"#,
            "glab: invalid_token",
        );
        assert!(matches!(
            answer(&expired, "gitlab.com"),
            Err(Failure::NotLoggedIn)
        ));
        let unauthorized = output(false, "", "glab: 401 Unauthorized");
        assert!(matches!(
            answer(&unauthorized, "gitlab.com"),
            Err(Failure::NotLoggedIn)
        ));
    }

    #[test]
    fn real_gitlab_errors_stay_errors() {
        // A GraphQL error while logged in: GitLab's own answer, read as such.
        let wrong = output(
            false,
            r#"{"errors":[{"message":"Field 'nope' doesn't exist on type 'CurrentUser'"}]}"#,
            "glab: Field 'nope' doesn't exist on type 'CurrentUser'",
        );
        let Ok(value) = answer(&wrong, "gitlab.com") else {
            panic!("GitLab's answer");
        };
        assert!(parse(&value, "gitlab.com")
            .unwrap_err()
            .starts_with("GitLab: Field"));
        let offline = output(
            false,
            "",
            "ERROR\n  Post \"https://git.acme.test/api/graphql\": dial tcp: lookup git.acme.test: no such host.",
        );
        assert!(matches!(
            answer(&offline, "git.acme.test"),
            Err(Failure::Other(e)) if e == "Cannot reach git.acme.test: check the connection"
        ));
    }

    #[test]
    fn an_answer_without_a_user_is_not_logged_in() {
        // glab has no token for a self-hosted host and asks anonymously.
        assert!(anonymous(&json!({"data": {"currentUser": null}})));
        assert!(anonymous(
            &json!({"data": {"currentUser": null, "projects": {"nodes": []}}})
        ));
        assert!(!anonymous(
            &json!({"data": {"currentUser": {"username": "me"}}})
        ));
        // An error is GitLab's to say, and a query without a user can't tell.
        assert!(!anonymous(
            &json!({"errors": [{"message": "boom"}], "data": {"currentUser": null}})
        ));
        assert!(!anonymous(&json!({"data": {"project": null}})));
        // Every list query asks who you are, so an anonymous answer shows.
        for status in [Status::Open, Status::Merged, Status::Closed] {
            let state = state_word(status);
            for scoped in [false, true] {
                assert!(
                    list_query("authoredMergeRequests", status, scoped).contains("currentUser {")
                );
            }
            assert!(PROJECT_ALL
                .replace("STATE", state)
                .contains("currentUser { username }"));
            assert!(MEMBER_PROJECTS
                .replace("STATE", state)
                .contains("currentUser { username }"));
            assert!(member_query(&["acme/app".into()], None, status)
                .contains("currentUser { username }"));
        }
    }

    #[test]
    fn an_error_that_mentions_401_is_not_a_login() {
        // Logged-in queries that GitLab refuses for another reason.
        for message in [
            "Query has complexity of 401, which exceeds max complexity of 250",
            "Jira integration: 401 Unauthorized",
            "Invalid token for webhook",
        ] {
            let out = output(
                false,
                &json!({"errors": [{"message": message}]}).to_string(),
                &format!("glab: {message}"),
            );
            assert!(answer(&out, "gitlab.com").is_ok(), "{message}");
        }
        // No network to a host whose name holds 401.
        let offline = output(
            false,
            "",
            "dial tcp: lookup gitlab401.acme.test: no such host",
        );
        assert!(matches!(
            answer(&offline, "gitlab401.acme.test"),
            Err(Failure::Other(e)) if e == "Cannot reach gitlab401.acme.test: check the connection"
        ));
        // GitLab's refusal is one also when glab exits without an error, but
        // not as one error next to the data of an answer.
        let quiet = output(true, r#"{"errors":[{"message":"Invalid token"}]}"#, "");
        assert!(matches!(
            answer(&quiet, "gitlab.com"),
            Err(Failure::NotLoggedIn)
        ));
        let partial = output(
            true,
            r#"{"errors":[{"message":"Invalid token"}],"data":{"currentUser":{"username":"me"}}}"#,
            "",
        );
        assert!(answer(&partial, "gitlab.com").is_ok());
    }

    #[test]
    fn an_error_wins_over_being_logged_out() {
        let timeout =
            || Err::<Option<Value>, String>("gitlab.com did not answer within 30 s".into());
        assert_eq!(
            both_lists(Ok(None), timeout()).unwrap_err(),
            "gitlab.com did not answer within 30 s"
        );
        assert_eq!(
            both_lists(timeout(), Ok(None)).unwrap_err(),
            "gitlab.com did not answer within 30 s"
        );
        assert!(both_lists(Ok(None), Ok(Some(json!({})))).unwrap().is_none());
        assert_eq!(
            all_answers(vec![timeout(), Ok(None)]).unwrap_err(),
            "gitlab.com did not answer within 30 s"
        );
        assert!(all_answers(vec![Ok(Some(json!({}))), Ok(None)])
            .unwrap()
            .is_none());
        assert_eq!(
            all_answers(vec![Ok(Some(json!({})))])
                .unwrap()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn all_asks_for_the_state() {
        assert!(PROJECT_ALL
            .replace("STATE", state_word(Status::Merged))
            .contains("mergeRequests(state: merged, first: 100, sort: UPDATED_DESC)"));
        assert!(MEMBER_PROJECTS
            .replace("STATE", state_word(Status::Closed))
            .contains("mergeRequests(state: closed, first: 100"));
        let q = member_query(&["acme/app".to_string()], None, Status::Merged);
        assert!(q.contains("mergeRequests(state: merged, first: 100"), "{q}");
    }

    #[test]
    fn project_all_has_everyones_once_with_the_total() {
        let v = json!({"data": {"currentUser": {"username": "me"}, "project": {"fullPath": "acme/app", "mergeRequests": {"count": 154, "nodes": [
            node(3, "acme/app", "sam", None, "2026-11-08T07:00:00Z"),
            node(5, "acme/app", "me", Some("alex"), "2026-11-08T09:00:00Z"),
            node(4, "acme/app", "alex", Some("me"), "2026-11-08T08:00:00Z"),
            node(3, "acme/app", "sam", None, "2026-11-08T07:00:00Z")
        ]}}}});
        let part = parse_project_all(&v, "gitlab.com").unwrap();
        let iids: Vec<u64> = part.items.iter().map(|m| m.iid).collect();
        assert_eq!(iids, [5, 4, 3], "newest first, each once");
        assert_eq!(part.total, 154, "GitLab's own count");
        assert_eq!(part.projects, ["gitlab.com/acme/app"]);
        assert!(part.items.iter().all(|m| m.viewer == "me"));
        assert_eq!(part.items[1].review_of("me"), Some(ReviewState::Unreviewed));
    }

    #[test]
    fn project_all_without_the_project_says_so() {
        let v = json!({"data": {"currentUser": {"username": "me"}, "project": null}});
        let e = parse_project_all(&v, "gitlab.com").unwrap_err();
        assert!(e.contains("did not return the project"), "{e}");
        let e =
            parse_project_all(&json!({"errors": [{"message": "boom"}]}), "gitlab.com").unwrap_err();
        assert!(e.contains("boom"));
    }

    /// The first step's answer: projects with the update times of their
    /// open merge requests.
    fn members(projects: &[(&str, u64, &[&str])]) -> Value {
        let nodes: Vec<Value> = projects
            .iter()
            .map(|(path, count, times)| {
                let mrs: Vec<Value> = times.iter().map(|t| json!({"updatedAt": t})).collect();
                json!({"fullPath": path, "mergeRequests": {"count": count, "nodes": mrs}})
            })
            .collect();
        json!({"data": {"projects": {"nodes": nodes}}})
    }

    #[test]
    fn member_pick_takes_the_newest_across_projects() {
        let v = members(&[
            (
                "acme/app",
                2,
                &["2026-11-08T09:00:00Z", "2026-11-01T09:00:00Z"],
            ),
            ("acme/lib", 1, &["2026-11-07T09:00:00Z"]),
            ("acme/docs", 0, &[]),
            ("acme/web", 1, &["2026-11-05T09:00:00Z"]),
        ]);
        let pick = pick_members(&v, 3).unwrap();
        assert_eq!(pick.paths, ["acme/app", "acme/lib", "acme/web"]);
        assert_eq!(pick.total, 4);
        assert_eq!(
            pick.members,
            ["acme/app", "acme/lib", "acme/docs", "acme/web"]
        );
        // One second before the oldest picked, so ties are kept.
        assert_eq!(pick.after.as_deref(), Some("2026-11-05T08:59:59Z"));
    }

    #[test]
    fn member_pick_loads_everything_under_the_limit() {
        let v = members(&[(
            "acme/app",
            2,
            &["2026-11-08T09:00:00Z", "2026-11-01T09:00:00Z"],
        )]);
        let pick = pick_members(&v, 100).unwrap();
        assert_eq!(pick.paths, ["acme/app"]);
        assert_eq!(pick.after, None);
        assert_eq!(pick.total, 2);
        // A project with more open than its first page is cut too.
        let v = members(&[("acme/app", 150, &["2026-11-08T09:00:00Z"])]);
        assert!(pick_members(&v, 100).unwrap().after.is_some());
        // A tie at the cut in a project not otherwise picked.
        let v = members(&[
            ("acme/app", 1, &["2026-11-08T09:00:00Z"]),
            ("acme/lib", 1, &["2026-11-07T09:00:00Z"]),
            ("acme/web", 1, &["2026-11-07T09:00:00Z"]),
        ]);
        let pick = pick_members(&v, 2).unwrap();
        assert_eq!(pick.paths, ["acme/app", "acme/lib", "acme/web"]);
        let empty = pick_members(&members(&[]), 100).unwrap();
        assert!(empty.paths.is_empty());
    }

    #[test]
    fn member_query_names_projects_and_time_as_strings() {
        let paths = vec!["acme/app".to_string(), "acme/\"lib".to_string()];
        let q = member_query(&paths, Some("2026-11-05T08:59:59Z"), Status::Open);
        assert!(
            q.contains(r#"fullPaths: ["acme/app", "acme/\"lib"], first: 2"#),
            "{q}"
        );
        assert!(q.contains(r#"updatedAfter: "2026-11-05T08:59:59Z""#), "{q}");
        assert!(q.contains("fragment mr on MergeRequest"));
        assert!(!member_query(&paths, None, Status::Open).contains("updatedAfter"));
    }

    #[test]
    fn members_parse_from_every_project() {
        let v = json!({"data": {"currentUser": {"username": "me"}, "projects": {"nodes": [
            {"mergeRequests": {"nodes": [node(1, "acme/app", "sam", None, "2026-11-08T09:00:00Z")]}},
            {"mergeRequests": {"nodes": [node(1, "acme/lib", "alex", Some("me"), "2026-11-08T10:00:00Z")]}}
        ]}}});
        let items = parse_members(&v, "gitlab.com").unwrap();
        let projects: Vec<&str> = items.iter().map(|m| m.project.as_str()).collect();
        assert_eq!(projects, ["acme/app", "acme/lib"]);
        let part = newest(items, 5, 1);
        assert_eq!(part.items[0].project, "acme/lib", "newest first, cut");
        assert_eq!(part.total, 5);
    }

    #[test]
    fn graphql_errors_are_reported() {
        let e = parse(&json!({"errors": [{"message": "boom"}]}), "gitlab.com").unwrap_err();
        assert!(e.contains("boom"));
    }
}
