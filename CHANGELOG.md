# Changelog

All notable changes to this plugin are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

### Changed

- "Mine" is now the first section, before "To review", and the list opens on
  it; `1` selects "Mine" and `2` "To review".

### Fixed

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
