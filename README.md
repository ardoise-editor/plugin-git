# Git — an Ardoise Editor plugin

Git in [Ardoise Editor](https://github.com/ardoise-editor/ardoise), in a panel under the editor:

- **Commit**: the staged and changed files, stage, unstage, roll back, delete, stash or ignore a file, commit (amend, sign-off, skip the hooks, author), commit and push, undo the last commit; the conflicts of a merge, rebase, cherry-pick or revert, and continue, skip or abort it.
- **Stash**: stash (keeping the index, or the index only), apply, pop, drop, make a branch of a stash, its files.
- **Log**: the history with its graph, of the current branch, all branches, a branch or tag, the reflog, or a file; search by message, author, content, file, date; filters; the commits picked: checkout, branch, tag, reset, revert, cherry-pick, reword, squash, drop, fixup, interactive rebase, compare, patch; the files of a commit: get them, apply them, take them out, move them to a commit of their own.
- **Branches**: local and remote branches and tags; checkout, merge, rebase, pull, fetch, push, force push, rename, delete, compare with the working tree.
- **Tools**: the current file (compare with or get from a revision, history of the file or of the selected lines), push with options, remotes, patches, merge and rebase with options, clean, submodules (added and updated) and worktrees (made and removed), bisect, the identity and the signing of a repository, clone, the repositories of the projects (fetch, pull, push them all).

In the editor: the branches and commits in Search Everywhere (Plugins tab), the colors of changed files in the tree and on the tabs, the changed lines marked beside the text, the blame (`Git: Annotate`), diffs in tabs where blocks are staged or put back, and conflicts settled in a tab.

Its settings (Preferences › Plugins) say how Pull and merges are made, and how often to fetch.

It needs `git` installed.

## Build

```sh
cargo test
cargo build --release && mkdir -p bin && cp target/release/git bin/
```

## License

MIT
