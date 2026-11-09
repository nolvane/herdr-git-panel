//! Runs the forges' own command line tools (`glab`, `gh`), which are already
//! logged in; the plugin never sees a token.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Longest wait for a server before the attempt counts as failed.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// A tool from PATH, else from the usual install places (Herdr may start the
/// plugin with a short PATH).
pub fn find(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in [
        format!("{home}/.homebrew/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        format!("{home}/.local/bin"),
        "/home/linuxbrew/.linuxbrew/bin".into(),
    ] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// A command for the tool, set up to never ask anything or check for
/// updates: its stdin is not the list's terminal.
pub fn command(name: &str) -> Command {
    let mut cmd = Command::new(find(name).unwrap_or_else(|| PathBuf::from(name)));
    cmd.env("GLAB_NO_PROMPT", "1")
        .env("GLAB_CHECK_UPDATE", "false")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    cmd
}

/// What a finished tool left behind.
pub struct Output {
    pub success: bool,
    /// The exit code, when the tool exited normally.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Runs the command with the usual time limit. `host` only names the server
/// in messages.
pub fn run(name: &str, cmd: Command, host: &str) -> Result<Output, String> {
    run_for(name, cmd, host, TIMEOUT)
}

/// Like [`run`], with `input` on the tool's standard input, such as a
/// request body for `--input -`: a long text there meets no limit on the
/// length of one argument and does not show in the process list.
pub fn run_input(name: &str, cmd: Command, host: &str, input: Vec<u8>) -> Result<Output, String> {
    run_with(name, cmd, host, TIMEOUT, Some(input))
}

/// Runs the command with this time limit. When it runs out, the tool is
/// stopped together with its process group, if it leads one (git with the
/// ssh it started).
pub fn run_for(name: &str, cmd: Command, host: &str, limit: Duration) -> Result<Output, String> {
    run_with(name, cmd, host, limit, None)
}

fn run_with(
    name: &str,
    mut cmd: Command,
    host: &str,
    limit: Duration,
    input: Option<Vec<u8>>,
) -> Result<Output, String> {
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                format!("{name} not found ({e}). Install it: brew install {name}")
            }
            // Such as a text too long for one argument.
            _ => format!("{name} could not start: {e}"),
        })?;

    // Write the input on a thread too, and close it so the tool reads to
    // the end; a tool that stops early only ends the write.
    if let (Some(data), Some(mut pipe)) = (input, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = std::io::Write::write_all(&mut pipe, &data);
        });
    }
    // Read the pipes on threads so a large answer cannot block the tool.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > limit => {
                // A group with the child's id exists only when the child
                // leads it; otherwise this finds no process and does nothing.
                #[cfg(unix)]
                let _ = Command::new("kill")
                    .args(["-s", "KILL", "--", &format!("-{}", child.id())])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{host} did not answer within {} s",
                    limit.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("{name}: {e}")),
        }
    };
    Ok(Output {
        success: status.success(),
        code: status.code(),
        stdout: out_reader.join().unwrap_or_default(),
        stderr: String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned(),
    })
}

/// The forges' whole answers when they refuse the token: GitHub's "Bad
/// credentials" and "Requires authentication" (a request without one),
/// GitLab's "401 Unauthorized" (REST), "Invalid token" (GraphQL) and
/// `invalid_token` (an expired OAuth token).
const REFUSALS: [&str; 5] = [
    "bad credentials",
    "requires authentication",
    "401 unauthorized",
    "invalid token",
    "invalid_token",
];

/// Whether the tool's error output says it is not logged in: the HTTP
/// status 401 as gh and glab write it ("(HTTP 401)", "HTTP 401:", glab's
/// "<url>: 401 {message: …}" and "status code: 401"), the tools' own words, or a refusal as the
/// whole message of a line. Not a bare "401" or "unauthorized": a number,
/// a name or an integration's message may hold those (#29).
pub fn not_logged_in(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    // glab's client libraries: "<METHOD> <url>: 401 …" and "status code:
    // 401"; a "401" after anything else is not the status.
    let after_url = lower.match_indices(": 401").any(|(i, _)| {
        lower[..i]
            .rsplit(char::is_whitespace)
            .next()
            .is_some_and(|w| w.starts_with("http"))
    });
    let status = after_url
        || ["http 401", "status code: 401"]
            .iter()
            .any(|w| lower.contains(w));
    let words = ["not logged in", "not authenticated", "auth login"]
        .iter()
        .any(|w| lower.contains(w));
    let whole = lower.lines().any(|line| {
        let mut line = line.trim();
        while let Some(rest) = ["glab:", "gh:", "error:"]
            .iter()
            .find_map(|p| line.strip_prefix(p))
        {
            line = rest.trim();
        }
        REFUSALS.contains(&line.trim_end_matches('.'))
    });
    status || words || whole
}

/// Whether a forge's JSON answer refuses the token: one of [`REFUSALS`] as
/// its whole `message`, `error` or GraphQL error message. Whole messages
/// only, so an error that merely mentions 401 or a token (a query's
/// complexity, an integration) stays an error.
pub fn refused(value: &serde_json::Value) -> bool {
    let is = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_str)
            .is_some_and(|m| REFUSALS.contains(&m.trim().to_lowercase().as_str()))
    };
    let errors = value.get("errors").and_then(serde_json::Value::as_array);
    errors.into_iter().flatten().any(|e| is(e.get("message")))
        || is(value.get("error"))
        || is(value.get("message"))
}

/// What to do about a tool that is not logged in to `host`. glab logs in
/// to gitlab.com unless it is told another host.
pub fn login_hint(name: &str, host: &str) -> String {
    let hostname = if name == "glab" && !host.eq_ignore_ascii_case("gitlab.com") {
        format!(" --hostname {host}")
    } else {
        String::new()
    };
    format!("{name} is not logged in to {host}: run {name} auth login{hostname}")
}

/// A one-line reason from a tool's error output, which may start with a
/// banner line such as "ERROR".
pub fn explain_failure(name: &str, host: &str, stderr: &str) -> String {
    let lower = stderr.to_lowercase();
    // First: a host name may hold "401", and no network is never a login.
    if lower.contains("no such host") || lower.contains("dial tcp") {
        return format!("Cannot reach {host}: check the connection");
    }
    if not_logged_in(stderr) {
        return login_hint(name, host);
    }
    let reason = stderr
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty() && !l.eq_ignore_ascii_case("error") && !l.starts_with("DEPRECATION")
        })
        .unwrap_or("unknown error");
    format!("{name} failed: {reason}")
}

/// Why a failed REST call (`glab api`, `gh api`) failed: the forge's JSON
/// answer decides when there is one, a refusal of the token as the login
/// hint and any other message as the forge's own words (`label` names the
/// forge); else the tool's error output.
pub fn api_error(name: &str, host: &str, label: &str, out: &Output) -> String {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
        if refused(&value) {
            return login_hint(name, host);
        }
        if let Some(msg) = rest_message(&value) {
            return format!("{label}: {msg}");
        }
    }
    explain_failure(name, host, &out.stderr)
}

/// The text of a REST error answer: GitLab's `message` (a string, or an
/// object of fields with their problems) and `error`, GitHub's `message`
/// with its `errors`.
fn rest_message(value: &serde_json::Value) -> Option<String> {
    use serde_json::Value;
    fn text(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => {
                let parts: Vec<String> = a.iter().filter_map(text).collect();
                (!parts.is_empty()).then(|| parts.join("; "))
            }
            Value::Object(o) => {
                if let Some(m) = o.get("message").and_then(text) {
                    return Some(m);
                }
                let parts: Vec<String> = o
                    .iter()
                    .filter_map(|(k, v)| text(v).map(|t| format!("{k} {t}")))
                    .collect();
                (!parts.is_empty()).then(|| parts.join("; "))
            }
            _ => None,
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for key in ["message", "error"] {
        if let Some(m) = value.get(key).and_then(text) {
            parts.push(m);
        }
    }
    if let Some(e) = value.get("errors").and_then(text) {
        parts.push(e);
    }
    (!parts.is_empty()).then(|| parts.join(": "))
}

/// The messages of a GraphQL `errors` list, joined.
pub fn graphql_errors(value: &serde_json::Value) -> Option<String> {
    let errors = value.get("errors")?.as_array()?;
    Some(
        errors
            .iter()
            .filter_map(|e| e.get("message").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_tool_out_of_time_is_stopped_with_its_group() {
        use std::os::unix::process::CommandExt;
        let pidfile =
            std::env::temp_dir().join(format!("herdr-git-panel-pid-{}", std::process::id()));
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep 30 & echo $! > {}; wait", pidfile.display()))
            .process_group(0);
        let started = Instant::now();
        let e = run_for("sh", cmd, "example.org", Duration::from_millis(300))
            .err()
            .unwrap();
        assert_eq!(e, "example.org did not answer within 0 s");
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        std::fs::remove_file(&pidfile).ok();
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the sleep it started is gone too");
    }

    fn failed(stdout: &str, stderr: &str) -> Output {
        Output {
            success: false,
            code: Some(1),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn rest_errors_say_what_the_forge_said() {
        let e = api_error(
            "glab",
            "gitlab.com",
            "GitLab",
            &failed(
                r#"{"message":{"note":["is too long (maximum is 1000000 characters)"]}}"#,
                "glab: 400 Bad Request",
            ),
        );
        assert_eq!(
            e,
            "GitLab: note is too long (maximum is 1000000 characters)"
        );
        let e = api_error(
            "glab",
            "gitlab.com",
            "GitLab",
            &failed(r#"{"message":"403 Forbidden"}"#, "glab: 403 Forbidden"),
        );
        assert_eq!(e, "GitLab: 403 Forbidden");
        let e = api_error(
            "gh",
            "github.com",
            "GitHub",
            &failed(
                r#"{"message":"Validation Failed","errors":[{"resource":"IssueComment","code":"custom","field":"body","message":"body is too long (maximum is 65536 characters)"}],"status":"422"}"#,
                "gh: Validation Failed (HTTP 422)",
            ),
        );
        assert_eq!(
            e,
            "GitHub: Validation Failed: body is too long (maximum is 65536 characters)"
        );
        // Not logged in, or no JSON: the tool's own words.
        let e = api_error(
            "gh",
            "github.com",
            "GitHub",
            &failed(
                r#"{"message":"Bad credentials"}"#,
                "gh: Bad credentials (HTTP 401)",
            ),
        );
        assert_eq!(e, "gh is not logged in to github.com: run gh auth login");
        let e = api_error(
            "glab",
            "x.com",
            "GitLab",
            &failed("", "dial tcp: lookup x.com: no such host"),
        );
        assert_eq!(e, "Cannot reach x.com: check the connection");
    }

    #[test]
    fn only_a_refused_login_is_not_logged_in() {
        // What gh and glab write when the login is missing or refused.
        for stderr in [
            "To get started with GitHub CLI, please run:  gh auth login",
            "gh: Bad credentials (HTTP 401)",
            "HTTP 401: Bad credentials (https://api.github.com/graphql)\nTry authenticating with:  gh auth login -h github.com",
            "glab: 401 Unauthorized",
            "glab: Invalid token",
            "glab: invalid_token",
            "ERROR\n\n  401 Unauthorized\n",
            "PUT https://gitlab.com/api/v4/projects/acme%2Fapp/merge_requests/7/merge: 401 {message: 401 Unauthorized}",
            "glab: HTTP 401",
            "error: glab: 401 Unauthorized",
            "POST https://git.acme.test/api/graphql: 401 failed to parse unknown error format: <html>",
            "non-200 OK status code: 401 Unauthorized body: \"\"",
        ] {
            assert!(not_logged_in(stderr), "{stderr}");
        }
        // Errors that only mention 401 or "unauthorized" stay errors.
        for stderr in [
            "gh: Query has complexity of 401, which exceeds max complexity of 400",
            "glab: merge request !401 cannot be merged",
            "glab: Jira integration: 401 Unauthorized",
            "gh: Unauthorized to push to protected branch",
            "GraphQL: Could not resolve to a Repository with the name 'acme/app401'.",
            "dial tcp: lookup git401.acme.test: no such host",
        ] {
            assert!(!not_logged_in(stderr), "{stderr}");
        }
    }

    #[test]
    fn writes_that_mention_401_say_what_failed() {
        assert_eq!(
            explain_failure(
                "glab",
                "gitlab.com",
                "glab: merge request !401 cannot be merged"
            ),
            "glab failed: glab: merge request !401 cannot be merged"
        );
        assert_eq!(
            explain_failure(
                "glab",
                "gitlab.com",
                "ERROR\n  Jira integration: 401 Unauthorized"
            ),
            "glab failed: Jira integration: 401 Unauthorized"
        );
        assert_eq!(
            explain_failure(
                "gh",
                "github.com",
                "GraphQL: Pull request #401 is not mergeable (mergePullRequest)"
            ),
            "gh failed: GraphQL: Pull request #401 is not mergeable (mergePullRequest)"
        );
        // A real refusal still says how to log in, with the host when it
        // is not gitlab.com.
        assert_eq!(
            explain_failure(
                "gh",
                "github.com",
                "HTTP 401: Bad credentials (https://api.github.com/graphql)"
            ),
            "gh is not logged in to github.com: run gh auth login"
        );
        assert_eq!(
            explain_failure("glab", "git.acme.test", "glab: 401 Unauthorized"),
            "glab is not logged in to git.acme.test: run glab auth login --hostname git.acme.test"
        );
        // No network to a host with 401 in its name is no login problem.
        assert_eq!(
            explain_failure(
                "glab",
                "git401.acme.test",
                "dial tcp: lookup git401.acme.test: no such host"
            ),
            "Cannot reach git401.acme.test: check the connection"
        );
    }

    #[test]
    fn rest_errors_that_mention_401_are_the_forges_words() {
        let e = api_error(
            "glab",
            "gitlab.com",
            "GitLab",
            &failed(
                r#"{"message":"Jira integration: 401 Unauthorized"}"#,
                "glab: Jira integration: 401 Unauthorized (HTTP 422)",
            ),
        );
        assert_eq!(e, "GitLab: Jira integration: 401 Unauthorized");
        let e = api_error(
            "gh",
            "github.com",
            "GitHub",
            &failed(
                r#"{"message":"Issue #401 is locked"}"#,
                "gh: Issue #401 is locked (HTTP 403)",
            ),
        );
        assert_eq!(e, "GitHub: Issue #401 is locked");
        // GitHub without a token answers "Requires authentication" (401).
        let e = api_error(
            "gh",
            "github.com",
            "GitHub",
            &failed(
                r#"{"message":"Requires authentication","status":"401"}"#,
                "gh: Requires authentication (HTTP 401)",
            ),
        );
        assert_eq!(e, "gh is not logged in to github.com: run gh auth login");
        // GitLab's refusal as its whole answer still says how to log in.
        let e = api_error(
            "glab",
            "git.acme.test",
            "GitLab",
            &failed(
                r#"{"message":"401 Unauthorized"}"#,
                "glab: 401 Unauthorized",
            ),
        );
        assert_eq!(
            e,
            "glab is not logged in to git.acme.test: run glab auth login --hostname git.acme.test"
        );
    }

    #[test]
    fn input_reaches_the_tool_whole() {
        let mut cmd = Command::new("wc");
        cmd.arg("-c");
        let text = "é".repeat(200_000);
        let out = run_input("wc", cmd, "example.org", text.clone().into_bytes()).unwrap();
        assert!(out.success);
        let count: usize = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
        assert_eq!(count, text.len());
    }

    #[test]
    fn a_tool_that_cannot_start_says_why() {
        let e = run_for(
            "sh",
            Command::new("/nonexistent/sh"),
            "example.org",
            Duration::from_secs(1),
        )
        .err()
        .unwrap();
        assert!(e.starts_with("sh not found"), "{e}");
        let mut cmd = Command::new("sh");
        cmd.arg("x".repeat(4 * 1024 * 1024));
        let e = run_for("sh", cmd, "example.org", Duration::from_secs(1))
            .err()
            .unwrap();
        assert!(e.starts_with("sh could not start: "), "{e}");
    }

    #[test]
    fn failures_are_explained() {
        let e = explain_failure(
            "glab",
            "x.com",
            "   ERROR  \n\n  Post \"https://x.com/api/graphql\": dial tcp: lookup x.com: no such host.",
        );
        assert_eq!(e, "Cannot reach x.com: check the connection");
        let e = explain_failure("glab", "x.com", "ERROR\n  something odd\n");
        assert_eq!(e, "glab failed: something odd");
        assert!(explain_failure(
            "gh",
            "github.com",
            "To get started with GitHub CLI, please run:  gh auth login"
        )
        .contains("gh auth login"));
        // GitLab refusing the token (none, revoked or expired).
        for stderr in ["glab: Invalid token", "glab: invalid_token"] {
            assert_eq!(
                explain_failure("glab", "gitlab.com", stderr),
                "glab is not logged in to gitlab.com: run glab auth login"
            );
        }
        // A self-hosted GitLab needs its host, or glab logs in to gitlab.com.
        assert_eq!(
            login_hint("glab", "git.acme.test"),
            "glab is not logged in to git.acme.test: run glab auth login --hostname git.acme.test"
        );
        assert_eq!(
            login_hint("glab", "GitLab.com"),
            "glab is not logged in to GitLab.com: run glab auth login"
        );
        assert_eq!(
            login_hint("gh", "github.com"),
            "gh is not logged in to github.com: run gh auth login"
        );
    }
}
