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

/// Runs the command with this time limit. When it runs out, the tool is
/// stopped together with its process group, if it leads one (git with the
/// ssh it started).
pub fn run_for(
    name: &str,
    mut cmd: Command,
    host: &str,
    limit: Duration,
) -> Result<Output, String> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{name} not found ({e}). Install it: brew install {name}"))?;

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

/// Whether the tool's error output says it is not logged in.
pub fn not_logged_in(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("not authenticated")
        || lower.contains("auth login")
        || lower.contains("not logged in")
}

/// A one-line reason from a tool's error output, which may start with a
/// banner line such as "ERROR".
pub fn explain_failure(name: &str, host: &str, stderr: &str) -> String {
    let lower = stderr.to_lowercase();
    if not_logged_in(stderr) {
        return format!("{name} is not logged in to {host}: run {name} auth login");
    }
    if lower.contains("no such host") || lower.contains("dial tcp") {
        return format!("Cannot reach {host}: check the connection");
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
    }
}
