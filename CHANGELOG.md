# Changelog

All notable changes to this plugin are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `Ctrl+r` starts a failed pipeline again, after `y`: on GitLab the failed
  jobs of the merge request's head pipeline, on GitHub the failed jobs of
  every failed GitHub Actions run of the newest commit. It starts nothing
  when someone pushed since the list loaded, and says why when there is
  nothing to start or the forge refuses. The details panel shows the key
  next to a failed pipeline (#12).

- `!` requests changes on someone else's open merge request or pull
  request, with a reason written in the same box as replies and comments
  (`Ctrl+s` shows it, only `y` sends it). On GitHub it is a review with
  "Request changes" on the commit shown; on GitLab a comment that submits
  your review as "requested changes". A line of the reason that starts
  with `/` never runs as a GitLab command (#11).

- A third section "All" lists every open merge request or pull request of
  the project, also other people's where you are not a reviewer; with "all
  projects" those of the projects you are a member of. `3` selects it, and
  `Tab` `→` `l` and `Shift+Tab` `←` `h` now step forward and back through
  the three sections (#6).

- `z` (or a click on the title) switches the list between open, merged and
  closed (without merging) merge requests and pull requests, in every
  section; the title says which. Merged and closed ones load the newest
  first, for this project only when the list shows this project, and can be
  looked at but not approved, merged, set to draft or opened as a branch
  (#7).

- `b` fetches the branch of the selected merge request or pull request and
  opens it in a new Herdr workspace (a git worktree Herdr creates), without
  touching the current checkout; forks open through the merge request's own
  ref (#9).

- In the activity tab, `>` replies to the selected thread and `+` writes a
  comment on the merge request or pull request, in a box that takes several
  lines (`Enter` line break, `Ctrl+s` shows what will be posted, `y` posts,
  `Esc` discards). The new reply or comment shows in the tab right away; a
  failed post keeps the text and says why. Pasted text goes into the box or
  the search in one piece (#10).

### Changed

- A refresh keeps the selected merge request or pull request selected when
  it moves in the list, instead of the row it was on (#10).
- The activity tab's key hints wrap instead of being cut off, and the key
  overview (`?`) uses two columns when it is taller than the screen (#10).

- "Mine" is now the first section, before "To review", and the list opens on
  it; `1` selects "Mine" and `2` "To review".

### Fixed

- With `glab` installed but not logged in, GitLab is now left out quietly,
  like GitHub without `gh`'s login, instead of showing "GitLab: Invalid
  token" in the footer on every refresh. When no forge can be used, the list
  names the tools that need `auth login`, for a self-hosted GitLab with
  `glab auth login --hostname <host>` (#19).
- The "Review" and "CI" dots and their headings start at the left edge of
  their column, like every other column, instead of sitting centred.

## [1.0.0] - 2026-11-08

### Added

- A list of the GitLab merge requests and GitHub pull requests you have to
  review and your own, opened over the tab or in a split, one per tab.
- Columns for your review, CI, approvals, open threads, changes and age, and
  a second line per row that says where it stands (draft, merge conflict,
  pipeline failed, ready to merge, …).
- A details panel with the description, labels and the activity (comments
  and threads), and a viewer for the changes with a file tree.
- Search, quick filters, sorting, this project or all projects, and mouse
  support.
- Approve, merge, resolve threads and mark your own as draft or ready from
  the list; approving and merging ask first and only act on the version that
  was on screen (#1).
- Only web links in descriptions and comments can be clicked (#1).
- Optional settings in `config.toml`, and plain symbols for terminals without
  a Nerd Font.

[Unreleased]: https://github.com/nolvane/herdr-git-panel/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/nolvane/herdr-git-panel/releases/tag/v1.0.0
