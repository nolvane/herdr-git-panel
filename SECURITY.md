# Security

Please report a vulnerability privately through GitHub:
**Security → Report a vulnerability** on this repository. Do not open a public
issue for it.

The plugin never sees a token: it asks GitLab and GitHub only through the
logged-in `glab` and `gh` command line tools. The most likely issues are in
how it handles what those tools return (titles, descriptions, comments,
diffs shown in the terminal) and in the few writes it makes on request
(approve, merge, resolve a thread, draft on or off).
