//! The changes of a merge request, parsed from a unified diff for the
//! built-in viewer: file headers, hunk headers and lines with their numbers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The start of a file: its path.
    File,
    /// `@@ -1,4 +1,6 @@ context`.
    Hunk,
    Added,
    Removed,
    Context,
    /// "new file", "deleted file", "Binary files differ" and the like.
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Index of the file's header line.
    pub at: usize,
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// A row of the file tree: a folder or a file, indented by its depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    pub name: String,
    /// Index into `files` for a file, `None` for a folder.
    pub file: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub lines: Vec<Line>,
    pub files: Vec<File>,
    pub added: usize,
    pub removed: usize,
}

fn line(kind: Kind, old: Option<u32>, new: Option<u32>, text: &str) -> Line {
    Line {
        kind,
        old,
        new,
        text: text.replace('\t', "    "),
    }
}

/// `-12,3` or `+7` from a hunk header to the first line number.
fn start(part: &str) -> u32 {
    part[1..]
        .split(',')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
}

pub fn parse(text: &str) -> Diff {
    let mut diff = Diff::default();
    let (mut old, mut new) = (0u32, 0u32);
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix("diff --git ") {
            // "a/path b/path": the new path is the part after " b/".
            let path = rest
                .rsplit_once(" b/")
                .map(|(_, p)| p.to_string())
                .unwrap_or_else(|| rest.to_string());
            diff.files.push(File {
                at: diff.lines.len(),
                path: path.clone(),
                added: 0,
                removed: 0,
            });
            diff.lines.push(line(Kind::File, None, None, &path));
        } else if raw.starts_with("@@") {
            let mut parts = raw.split_whitespace().skip(1);
            old = parts.next().map(start).unwrap_or(1);
            new = parts.next().map(start).unwrap_or(1);
            diff.lines.push(line(Kind::Hunk, None, None, raw));
        } else if raw.starts_with("+++") || raw.starts_with("---") || raw.starts_with("index ") {
            continue;
        } else if let Some(t) = raw.strip_prefix('+') {
            diff.lines.push(line(Kind::Added, None, Some(new), t));
            diff.added += 1;
            if let Some(f) = diff.files.last_mut() {
                f.added += 1;
            }
            new += 1;
        } else if let Some(t) = raw.strip_prefix('-') {
            diff.lines.push(line(Kind::Removed, Some(old), None, t));
            diff.removed += 1;
            if let Some(f) = diff.files.last_mut() {
                f.removed += 1;
            }
            old += 1;
        } else if let Some(t) = raw.strip_prefix(' ') {
            diff.lines
                .push(line(Kind::Context, Some(old), Some(new), t));
            old += 1;
            new += 1;
        } else if raw.starts_with('\\') || raw.is_empty() {
            continue;
        } else {
            diff.lines.push(line(Kind::Note, None, None, raw));
        }
    }
    diff
}

/// The changed files as a tree: folders once, their files indented below,
/// in the diff's order. Single-child folder chains are joined (`a/b/c`).
pub fn tree(files: &[File]) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let mut open: Vec<String> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let (dir, name) = match f.path.rsplit_once('/') {
            Some((d, n)) => (d.to_string(), n.to_string()),
            None => (String::new(), f.path.clone()),
        };
        let parts: Vec<String> = if dir.is_empty() { vec![] } else { vec![dir] };
        // One folder row per distinct folder; files under it.
        if open != parts {
            if let Some(d) = parts.first() {
                rows.push(TreeRow {
                    depth: 0,
                    name: format!("{d}/"),
                    file: None,
                });
            }
            open = parts.clone();
        }
        rows.push(TreeRow {
            depth: if parts.is_empty() { 0 } else { 1 },
            name,
            file: Some(i),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_files_hunks_and_numbers() {
        let text = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -10,3 +10,4 @@ fn x()\n keep\n-old\n+new\n+more\n\\ No newline at end of file\ndiff --git a/b.md b/b.md\nnew file mode 100644\n@@ -0,0 +1 @@\n+hi\n";
        let d = parse(text);
        let files: Vec<(usize, &str, usize, usize)> = d
            .files
            .iter()
            .map(|f| (f.at, f.path.as_str(), f.added, f.removed))
            .collect();
        assert_eq!(files, vec![(0, "src/a.rs", 2, 1), (6, "b.md", 1, 0)]);
        let t = tree(&d.files);
        let names: Vec<(usize, &str)> = t.iter().map(|r| (r.depth, r.name.as_str())).collect();
        assert_eq!(names, vec![(0, "src/"), (1, "a.rs"), (0, "b.md")]);
        assert_eq!((d.added, d.removed), (3, 1));
        let kinds: Vec<Kind> = d.lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::File,
                Kind::Hunk,
                Kind::Context,
                Kind::Removed,
                Kind::Added,
                Kind::Added,
                Kind::File,
                Kind::Note,
                Kind::Hunk,
                Kind::Added
            ]
        );
        assert_eq!((d.lines[2].old, d.lines[2].new), (Some(10), Some(10)));
        assert_eq!(d.lines[3].old, Some(11));
        assert_eq!(d.lines[4].new, Some(11));
        assert_eq!(d.lines[9].new, Some(1));
    }
}
