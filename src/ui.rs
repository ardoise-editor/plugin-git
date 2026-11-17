//! The Git panel as nodes the editor draws.

use simpleedit_plugin::ui::{
    button, checkbox, column, editor, input, item, list, row, scroll, select, separator, space,
    text, tree, tree_item, ButtonStyle, Item, Node, TextStyle, TreeItem,
};

use crate::git::history::{PlanAction, SearchMode};
use crate::git::tools::PushTags;
use crate::git::{Change, Kind, Operation};
use crate::state::{self, Dialog, Git, RefDialog, Settings, Tab, Tool};

type Label = fn() -> String;

pub const PULL_MODES: [(&str, Label); 3] = [
    ("merge", || t!("git.pull_merge").to_string()),
    ("rebase", || t!("git.pull_rebase").to_string()),
    ("ff-only", || t!("git.pull_ff_only").to_string()),
];

pub const MERGE_MODES: [(&str, Label); 4] = [
    ("default", || t!("git.merge_default").to_string()),
    ("no-ff", || t!("git.merge_no_ff").to_string()),
    ("ff-only", || t!("git.merge_ff_only").to_string()),
    ("squash", || t!("git.merge_squash").to_string()),
];

pub fn search_modes() -> Vec<(SearchMode, String)> {
    vec![
        (SearchMode::Message, t!("git.search_message").to_string()),
        (SearchMode::Author, t!("git.search_author").to_string()),
        (SearchMode::Content, t!("git.search_content").to_string()),
        (SearchMode::File, t!("git.search_file").to_string()),
        (SearchMode::Since, t!("git.search_since").to_string()),
    ]
}

pub fn reset_modes() -> Vec<(&'static str, String)> {
    vec![
        ("soft", t!("git.reset_text_soft").to_string()),
        ("mixed", t!("git.reset_text_mixed").to_string()),
        ("hard", t!("git.reset_text_hard").to_string()),
    ]
}

pub fn plan_actions() -> Vec<(PlanAction, String)> {
    vec![
        (PlanAction::Pick, t!("git.plan_pick").to_string()),
        (PlanAction::Reword, t!("git.plan_reword").to_string()),
        (PlanAction::Edit, t!("git.plan_edit").to_string()),
        (PlanAction::Squash, t!("git.plan_squash").to_string()),
        (PlanAction::Fixup, t!("git.plan_fixup").to_string()),
        (PlanAction::Drop, t!("git.plan_drop").to_string()),
    ]
}

pub fn push_tags() -> Vec<(PushTags, String)> {
    vec![
        (PushTags::None, t!("git.push_tags_none").to_string()),
        (PushTags::Current, t!("git.push_tags_current").to_string()),
        (PushTags::All, t!("git.push_tags_all").to_string()),
    ]
}

pub fn all_branches() -> String {
    t!("git.all_branches").to_string()
}

fn label_of<T: PartialEq>(options: &[(T, String)], value: &T) -> Option<String> {
    options
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| l.clone())
}

fn tool_button(label: impl Into<String>, action: &str, enabled: bool) -> Node {
    button(label, action)
        .button_style(ButtonStyle::Ghost)
        .enabled(enabled)
}

/// A button that asks to be clicked twice: the second click is told.
fn armed_button(g: &Git, label: String, action: &str, enabled: bool) -> Node {
    if g.armed.as_deref() == Some(action) {
        button(t!("git.branch_confirm").to_string(), action)
            .button_style(ButtonStyle::Danger)
            .enabled(enabled)
    } else {
        tool_button(label, action, enabled)
    }
}

/// The whole panel: the tabs, what is said, then the tab.
pub fn panel(g: &Git) -> Node {
    let free = g.busy.is_none();
    let tab = |label: String, action: &str, this: Tab| {
        let style = if g.tab == this {
            ButtonStyle::Primary
        } else {
            ButtonStyle::Ghost
        };
        button(label, action).button_style(style)
    };
    let mut header = vec![
        tab(t!("git.tab_commit").to_string(), "tab:commit", Tab::Commit),
        tab(t!("git.tab_stash").to_string(), "tab:stash", Tab::Stash),
        tab(t!("git.tab_log").to_string(), "tab:log", Tab::Log),
        tab(
            t!("git.tab_branches").to_string(),
            "tab:branches",
            Tab::Branches,
        ),
        tab(t!("git.tab_tools").to_string(), "tab:tools", Tab::Tools),
    ];
    if let Some(status) = &g.status {
        header.push(
            text(state::branch_label(
                &status.branch,
                status.ahead,
                status.behind,
            ))
            .style(TextStyle::Muted),
        );
    }
    if g.repo.is_some() {
        header.extend([
            tool_button(t!("git.refresh").to_string(), "refresh", free),
            tool_button(t!("git.fetch").to_string(), "fetch", free),
            tool_button(t!("git.pull").to_string(), "pull", free),
            tool_button(t!("git.push").to_string(), "push", free),
        ]);
    }
    let mut body = vec![row(header).spacing(4.0)];
    if let Some(busy) = &g.busy {
        body.push(text(format!("{busy}…")).style(TextStyle::Muted));
    }
    if let Some((notice, error)) = &g.notice {
        body.push(
            row([
                text(notice.clone()).style(if *error {
                    TextStyle::Error
                } else {
                    TextStyle::Muted
                }),
                tool_button("×", "dismiss", true),
            ])
            .spacing(6.0),
        );
    }
    body.push(separator());
    let content = match (&g.repo, g.tab) {
        (_, Tab::Tools) => tools_tab(g),
        (None, _) if g.searched => no_repository(g),
        (None, _) => space(0.0),
        (Some(_), Tab::Commit) => commit_tab(g),
        (Some(_), Tab::Stash) => stash_tab(g),
        (Some(_), Tab::Log) => log_tab(g),
        (Some(_), Tab::Branches) => branches_tab(g),
    };
    body.push(content);
    column(body).spacing(6.0).padding(8.0)
}

fn no_repository(g: &Git) -> Node {
    let init = match &g.init_dir {
        Some(dir) => t!("git.init_in", dir = dir.display().to_string()).to_string(),
        None => t!("git.init").to_string(),
    };
    column([
        text(t!("git.no_repo").to_string()).style(TextStyle::Muted),
        button(init, "init").enabled(g.busy.is_none() && g.init_dir.is_some()),
        tool_button(t!("git.clone_action").to_string(), "tab:tools", true),
    ])
    .spacing(8.0)
}

fn kind_letter(kind: Kind) -> &'static str {
    kind.letter()
}

fn change_items(changes: &[Change]) -> Vec<Item> {
    const LIMIT: usize = 1000;
    changes
        .iter()
        .take(LIMIT)
        .map(|c| {
            let label = match &c.from {
                Some(from) => format!("{from} → {}", c.path),
                None => c.path.clone(),
            };
            item(c.path.clone(), label).detail(kind_letter(c.kind))
        })
        .collect()
}

fn commit_tab(g: &Git) -> Node {
    let Some(status) = &g.status else {
        return space(0.0);
    };
    let free = g.busy.is_none();
    let mut parts = Vec::new();

    // A merge, rebase… stopped on conflicts.
    let conflicts = g.conflicts();
    if let Some(operation) = status.operation {
        let op = match operation {
            Operation::Merge => t!("git.op_merge"),
            Operation::Rebase => t!("git.op_rebase"),
            Operation::CherryPick => t!("git.op_cherry_pick"),
            Operation::Revert => t!("git.op_revert"),
        }
        .to_string();
        let said = if conflicts.is_empty() {
            t!("git.op_ready", op = op.as_str()).to_string()
        } else {
            t!("git.op_conflicts", op = op.as_str(), n = conflicts.len()).to_string()
        };
        parts.push(
            row([
                text(said).style(TextStyle::Bold),
                tool_button(
                    t!("git.continue").to_string(),
                    "op:continue",
                    free && conflicts.is_empty(),
                ),
                tool_button(t!("git.skip").to_string(), "op:skip", free),
                armed_button(g, t!("git.abort").to_string(), "confirm:op:abort", free),
            ])
            .spacing(6.0),
        );
    }
    if !conflicts.is_empty() {
        let picked = g.picked.as_ref().map(|(_, p)| p.clone());
        let items: Vec<Item> = conflicts
            .iter()
            .map(|c| item(c.path.clone(), c.path.clone()).detail("!"))
            .collect();
        let mut conflict_list = list("conflicts", items);
        if let Some(p) = &picked {
            conflict_list = conflict_list.selected(p.clone());
        }
        let on = picked
            .as_ref()
            .is_some_and(|p| conflicts.iter().any(|c| &c.path == p));
        parts.extend([
            text(t!("git.conflicts_title", n = conflicts.len()).to_string())
                .style(TextStyle::Heading),
            conflict_list,
            row([
                tool_button(
                    t!("git.accept_yours").to_string(),
                    "accept_yours",
                    free && on,
                ),
                tool_button(
                    t!("git.accept_theirs").to_string(),
                    "accept_theirs",
                    free && on,
                ),
                tool_button(
                    t!("git.merge_manually").to_string(),
                    "merge_file",
                    free && on,
                ),
            ])
            .spacing(6.0),
        ]);
    }

    // The files: staged, then the others.
    let picked = |staged: bool| {
        g.picked
            .as_ref()
            .filter(|(s, _)| *s == staged)
            .map(|(_, p)| p.clone())
    };
    let section = |staged: bool| {
        let changes: Vec<Change> = if staged {
            status.staged.clone()
        } else {
            status
                .unstaged
                .iter()
                .filter(|c| c.kind != Kind::Conflict)
                .cloned()
                .collect()
        };
        let (title, all, all_action) = if staged {
            (
                t!("git.staged", n = changes.len()).to_string(),
                t!("git.unstage").to_string(),
                "unstage_all",
            )
        } else {
            (
                t!("git.changes", n = changes.len()).to_string(),
                t!("git.stage").to_string(),
                "stage_all",
            )
        };
        let mut nodes = vec![row([
            text(title).style(TextStyle::Heading),
            tool_button(all, all_action, free && !changes.is_empty()),
        ])
        .spacing(6.0)];
        if changes.len() > 1000 {
            nodes.push(text(
                t!("git.too_many_files", n = changes.len() - 1000).to_string(),
            ));
        }
        let mut files = list(
            if staged { "staged" } else { "unstaged" },
            change_items(&changes),
        );
        if let Some(p) = picked(staged) {
            files = files.selected(p);
        }
        nodes.push(files);
        nodes
    };
    let mut lists = section(true);
    lists.extend(section(false));
    if status.staged.is_empty() && status.unstaged.is_empty() {
        lists.push(text(t!("git.nothing_to_commit").to_string()).style(TextStyle::Muted));
    }
    parts.push(scroll(column(lists).spacing(4.0)).height(220.0));

    // What can be done to the file picked.
    if let Some((staged, path)) = &g.picked {
        if let Some(change) = g.change(*staged, path) {
            let untracked = change.kind == Kind::Untracked;
            let mut actions = vec![text(path.clone()).style(TextStyle::Mono)];
            actions.push(if *staged {
                tool_button(t!("git.file_unstage").to_string(), "file:unstage", free)
            } else {
                tool_button(t!("git.file_stage").to_string(), "file:stage", free)
            });
            actions.extend([
                tool_button(t!("git.file_diff").to_string(), "file:diff", true),
                tool_button(t!("git.open_file").to_string(), "file:open", true),
                tool_button(
                    t!("git.file_history").to_string(),
                    "file:history",
                    !untracked,
                ),
                tool_button(t!("git.file_commit").to_string(), "file:commit_only", free),
            ]);
            if untracked {
                actions.push(armed_button(
                    g,
                    t!("git.delete").to_string(),
                    "confirm:file:delete",
                    free,
                ));
                actions.push(tool_button(
                    t!("git.ignore_file").to_string(),
                    "file:ignore",
                    free,
                ));
            } else {
                actions.push(armed_button(
                    g,
                    t!("git.rollback").to_string(),
                    "confirm:file:rollback",
                    free,
                ));
                actions.push(tool_button(
                    t!("git.stash_file").to_string(),
                    "file:stash",
                    free,
                ));
            }
            parts.push(row(actions).spacing(4.0));
        }
    }

    // The message, and committing.
    parts.push(separator());
    parts.push(
        editor("message", g.message.clone(), "")
            .placeholder(t!("git.message_placeholder").to_string())
            .height(90.0),
    );
    let mut commit_row = vec![
        checkbox("amend", t!("git.amend").to_string(), g.amend),
        tool_button(t!("git.commit_options").to_string(), "show_options", true),
    ];
    if !g.recent.is_empty() {
        commit_row.push(
            select(
                "recent",
                g.recent
                    .iter()
                    .map(|m| m.lines().next().unwrap_or_default().to_string()),
                None,
            )
            .placeholder(t!("git.recent_messages").to_string()),
        );
    }
    commit_row.extend([
        button(t!("git.commit").to_string(), "commit")
            .button_style(ButtonStyle::Primary)
            .enabled(free),
        button(t!("git.commit_push").to_string(), "commit_push").enabled(free),
        armed_button(
            g,
            t!("git.undo_commit").to_string(),
            "confirm:undo_commit",
            free,
        ),
    ]);
    parts.push(row(commit_row).spacing(6.0));
    if g.show_options {
        parts.push(
            row([
                checkbox("signoff", t!("git.signoff").to_string(), g.options.signoff),
                checkbox(
                    "no_verify",
                    t!("git.no_verify").to_string(),
                    g.options.no_verify,
                ),
                input(
                    "author",
                    g.options.author.clone(),
                    t!("git.author_placeholder").to_string(),
                ),
            ])
            .spacing(8.0),
        );
    }
    column(parts).spacing(6.0)
}

fn stash_tab(g: &Git) -> Node {
    let free = g.busy.is_none();
    let mut parts = vec![
        row([
            input(
                "stash_message",
                g.stash_message.clone(),
                t!("git.stash_message").to_string(),
            ),
            checkbox(
                "stash_keep_index",
                t!("git.stash_keep_index").to_string(),
                g.stash_keep_index,
            ),
            checkbox(
                "stash_staged",
                t!("git.stash_staged").to_string(),
                g.stash_staged,
            ),
            button(t!("git.stash_push").to_string(), "stash:push")
                .button_style(ButtonStyle::Primary)
                .enabled(free),
        ])
        .spacing(6.0),
        separator(),
    ];
    if g.stashes.is_empty() {
        parts.push(text(t!("git.no_stash").to_string()).style(TextStyle::Muted));
        return column(parts).spacing(6.0);
    }
    let items: Vec<Item> = g
        .stashes
        .iter()
        .map(|s| {
            item(
                s.index.to_string(),
                format!("stash@{{{}}}  {}", s.index, s.message),
            )
            .detail(format!("{} · {}", s.branch, state::format_date(s.time)))
        })
        .collect();
    let mut stashes = list("stashes", items);
    if let Some(i) = g.stash {
        stashes = stashes.selected(i.to_string());
    }
    parts.push(scroll(stashes).height(160.0));
    if g.stash.is_some() {
        parts.push(
            row([
                tool_button(t!("git.stash_apply").to_string(), "stash:apply", free),
                tool_button(t!("git.stash_pop").to_string(), "stash:pop", free),
                armed_button(
                    g,
                    t!("git.stash_drop").to_string(),
                    "confirm:stash:drop",
                    free,
                ),
                input(
                    "stash_branch",
                    g.stash_branch.clone(),
                    t!("git.stash_branch_name").to_string(),
                ),
                tool_button(t!("git.stash_branch").to_string(), "stash:branch", free),
            ])
            .spacing(6.0),
        );
        parts.push(list("stash_files", change_items(&g.stash_files)));
    }
    column(parts).spacing(6.0)
}

/// The lanes of the graph of a commit, as text.
fn graph_text(row: &crate::git::graph::GraphRow, width: usize, in_head: bool) -> String {
    let mut cells = vec![' '; width.max(row.lane + 1)];
    for (from, to) in row
        .through
        .iter()
        .chain(&row.from_above)
        .chain(&row.to_below)
    {
        for lane in [*from, *to] {
            if lane < cells.len() && cells[lane] == ' ' {
                cells[lane] = '│';
            }
        }
    }
    cells[row.lane] = if in_head { '●' } else { '○' };
    cells.into_iter().collect()
}

fn log_tab(g: &Git) -> Node {
    let free = g.busy.is_none();
    if let Some((_, plan)) = &g.plan {
        return rebase_plan(plan, free);
    }
    let mut revs = vec![
        t!("git.head_current").to_string(),
        all_branches(),
        t!("git.reflog").to_string(),
    ];
    revs.extend(
        g.refs
            .branches
            .iter()
            .map(|b| b.name.clone())
            .chain(g.refs.tags.iter().cloned()),
    );
    let shown = match g.log_rev.as_str() {
        "HEAD" => t!("git.head_current").to_string(),
        "--all" => all_branches(),
        crate::git::history::REFLOG => t!("git.reflog").to_string(),
        other => other.to_string(),
    };
    let modes = search_modes();
    let mut parts = vec![row([
        select("log_rev", revs, Some(shown)),
        input(
            "search",
            g.search.clone(),
            t!("git.search_placeholder").to_string(),
        )
        .on_submit("search_run"),
        select(
            "search_mode",
            modes.iter().map(|(_, l)| l.clone()),
            label_of(&modes, &g.search_mode),
        ),
        checkbox("several", t!("git.pick_several").to_string(), g.several),
        tool_button(t!("git.filters").to_string(), "show_filters", true),
    ])
    .spacing(6.0)];
    if g.show_filters {
        let f = &g.filter;
        parts.push(
            row([
                input(
                    "filter_author",
                    f.author.clone(),
                    t!("git.filter_author").to_string(),
                )
                .on_submit("filter:apply"),
                input(
                    "filter_since",
                    f.since.clone(),
                    t!("git.filter_since").to_string(),
                )
                .on_submit("filter:apply"),
                input(
                    "filter_until",
                    f.until.clone(),
                    t!("git.filter_until").to_string(),
                )
                .on_submit("filter:apply"),
                input(
                    "filter_path",
                    f.path.clone(),
                    t!("git.filter_path").to_string(),
                )
                .on_submit("filter:apply"),
            ])
            .spacing(6.0),
        );
        let mut line = vec![
            checkbox(
                "filter_first_parent",
                t!("git.filter_first_parent").to_string(),
                f.first_parent,
            ),
            checkbox(
                "filter_no_merges",
                t!("git.filter_no_merges").to_string(),
                f.no_merges,
            ),
            tool_button(t!("git.filter_apply").to_string(), "filter:apply", true),
            tool_button(t!("git.filter_clear").to_string(), "filter:clear", true),
        ];
        if let Some((path, first, last)) = &f.lines {
            let lines = format!("{path}:{first}-{last}");
            line.push(
                text(t!("git.lines_history_of", lines = lines.as_str()).to_string())
                    .style(TextStyle::Bold),
            );
        }
        parts.push(row(line).spacing(8.0));
    }
    if let Some(path) = &g.log_path {
        parts.push(
            row([
                text(t!("git.file_history_of", file = path.as_str()).to_string())
                    .style(TextStyle::Bold),
                tool_button(t!("git.filter_clear").to_string(), "log:clear_file", true),
            ])
            .spacing(6.0),
        );
    }
    if g.commits.is_empty() {
        parts.push(text(t!("git.no_commits").to_string()).style(TextStyle::Muted));
        return column(parts).spacing(6.0);
    }
    let width = crate::git::graph::width(&g.rows);
    let items: Vec<Item> = g
        .commits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let head = g.in_head.is_empty() || g.in_head.contains(&c.sha);
            let lanes = g
                .rows
                .get(i)
                .map(|r| graph_text(r, width, head))
                .unwrap_or_default();
            let mark = if g.several && g.selected.contains(&c.sha) {
                "☑ "
            } else {
                ""
            };
            let refs = if c.refs.is_empty() {
                String::new()
            } else {
                format!("[{}] ", c.refs.join(", "))
            };
            item(c.sha.clone(), format!("{lanes} {mark}{refs}{}", c.subject)).detail(format!(
                "{} · {} · {}",
                &c.sha[..c.sha.len().min(8)],
                c.author,
                state::format_date(c.time)
            ))
        })
        .collect();
    let mut commits = list("commits", items);
    if let Some(sha) = g.selected.last() {
        commits = commits.selected(sha.clone());
    }
    parts.push(scroll(commits).height(260.0));
    if g.commits.len() >= g.limit {
        parts.push(tool_button(
            t!("git.load_more").to_string(),
            "load_more",
            true,
        ));
    }
    if let Some(commit) = g.commit() {
        let n = g.selected.len();
        if n > 1 {
            parts.push(text(t!("git.n_selected", n = n).to_string()).style(TextStyle::Bold));
        }
        parts.push(
            row([
                tool_button(
                    t!("git.checkout").to_string(),
                    "log:checkout",
                    free && n == 1,
                ),
                tool_button(
                    t!("git.log_branch_here").to_string(),
                    "log:branch",
                    free && n == 1,
                ),
                tool_button(
                    t!("git.log_tag_here").to_string(),
                    "log:tag",
                    free && n == 1,
                ),
                tool_button(t!("git.reset").to_string(), "log:reset", free && n == 1),
                tool_button(t!("git.copy_hash").to_string(), "log:copy", n == 1),
                tool_button(t!("git.diff_tree").to_string(), "log:diff_tree", n == 1),
                tool_button(
                    t!("git.log_compare_two").to_string(),
                    "log:compare_two",
                    n == 2,
                ),
                tool_button(t!("git.patch_copy").to_string(), "log:patch", true),
            ])
            .spacing(2.0),
        );
        parts.push(
            row([
                armed_button(g, t!("git.revert").to_string(), "confirm:log:revert", free),
                armed_button(
                    g,
                    t!("git.cherry_pick").to_string(),
                    "confirm:log:cherry_pick",
                    free,
                ),
                tool_button(t!("git.reword").to_string(), "log:reword", free && n == 1),
                tool_button(t!("git.squash").to_string(), "log:squash", free && n > 1),
                armed_button(g, t!("git.drop").to_string(), "confirm:log:drop", free),
                tool_button(t!("git.fixup").to_string(), "log:fixup", free && n == 1),
                tool_button(
                    t!("git.squash_into").to_string(),
                    "log:squash_into",
                    free && n == 1,
                ),
                tool_button(
                    t!("git.log_rebase_from").to_string(),
                    "log:rebase_from",
                    free && n == 1,
                ),
            ])
            .spacing(2.0),
        );
        if let Some(dialog) = &g.dialog {
            parts.push(log_dialog(g, dialog, &commit.sha));
        }
        parts.push(
            text(format!(
                "{} <{}> · {}",
                commit.author,
                commit.email,
                state::format_time(commit.time)
            ))
            .style(TextStyle::Muted),
        );
        if let Some(signature) = &g.signature {
            parts.push(text(signature.clone()).style(TextStyle::Muted));
        }
        if !g.containing.is_empty() {
            let shown: Vec<&str> = g.containing.iter().take(8).map(String::as_str).collect();
            parts.push(
                text(
                    t!(
                        "git.in_branches",
                        n = g.containing.len(),
                        list = shown.join(", ")
                    )
                    .to_string(),
                )
                .style(TextStyle::Muted),
            );
        }
        parts.push(text(g.commit_message.clone()).style(TextStyle::Mono));
        let mut files = list("commit_files", change_items(&g.commit_files));
        if let Some(f) = &g.commit_file {
            files = files.selected(f.clone());
        }
        parts.push(files);
        if g.commit_file.is_some() && n == 1 {
            parts.push(
                row([
                    tool_button(t!("git.file_get").to_string(), "cfile:get", free),
                    tool_button(t!("git.file_apply").to_string(), "cfile:apply", free),
                    armed_button(
                        g,
                        t!("git.file_remove").to_string(),
                        "confirm:cfile:remove",
                        free,
                    ),
                    tool_button(t!("git.file_extract").to_string(), "cfile:extract", free),
                ])
                .spacing(4.0),
            );
        }
    }
    column(parts).spacing(6.0)
}

fn log_dialog(g: &Git, dialog: &Dialog, sha: &str) -> Node {
    let short = &sha[..sha.len().min(8)];
    let field = match dialog {
        Dialog::Branch => column([
            text(t!("git.name_branch_title", sha = short).to_string()).style(TextStyle::Bold),
            input(
                "dialog_text",
                g.dialog_text.clone(),
                t!("git.new_branch_name").to_string(),
            )
            .on_submit("dialog:ok"),
        ]),
        Dialog::Tag => column([
            text(t!("git.name_tag_title", sha = short).to_string()).style(TextStyle::Bold),
            editor("dialog_text", g.dialog_text.clone(), "")
                .placeholder(t!("git.tag_with_message").to_string())
                .height(70.0),
        ]),
        Dialog::Reword | Dialog::Squash | Dialog::Extract => {
            let title = match dialog {
                Dialog::Reword => t!("git.reword_title"),
                Dialog::Squash => t!("git.squash_title"),
                _ => t!("git.extract_message"),
            };
            let mut nodes = vec![
                text(title.to_string()).style(TextStyle::Bold),
                text(t!("git.rewrite_text").to_string()).style(TextStyle::Muted),
            ];
            if g.pushed {
                nodes.push(text(t!("git.pushed_warning").to_string()).style(TextStyle::Error));
            }
            nodes.push(editor("dialog_text", g.dialog_text.clone(), "").height(110.0));
            column(nodes)
        }
        Dialog::Reset => {
            let modes = reset_modes();
            let shown = modes
                .iter()
                .find(|(m, _)| *m == g.reset_mode)
                .map(|(_, l)| l.clone());
            column([
                text(t!("git.reset_title", sha = short).to_string()).style(TextStyle::Bold),
                select("reset_mode", modes.iter().map(|(_, l)| l.clone()), shown),
            ])
        }
    };
    column([
        field.spacing(4.0),
        row([
            button(t!("git.ok").to_string(), "dialog:ok")
                .button_style(ButtonStyle::Primary)
                .enabled(g.busy.is_none()),
            button(t!("git.cancel").to_string(), "dialog:cancel"),
        ])
        .spacing(6.0),
    ])
    .spacing(6.0)
}

fn rebase_plan(plan: &[crate::git::history::PlanItem], free: bool) -> Node {
    let actions = plan_actions();
    let mut rows = vec![text(t!("git.rebase_help").to_string()).style(TextStyle::Muted)];
    for (i, item) in plan.iter().enumerate() {
        let mut line = vec![
            tool_button("↑", &format!("plan_up:{i}"), i > 0),
            tool_button("↓", &format!("plan_down:{i}"), i + 1 < plan.len()),
            select(
                format!("plan_action:{i}"),
                actions.iter().map(|(_, l)| l.clone()),
                label_of(&actions, &item.action),
            ),
            text(item.sha[..item.sha.len().min(8)].to_string()).style(TextStyle::Mono),
        ];
        if item.action == PlanAction::Reword {
            line.push(input(format!("plan_subject:{i}"), item.message.clone(), ""));
        } else {
            line.push(text(item.subject.clone()));
        }
        rows.push(row(line).spacing(4.0));
    }
    rows.push(
        row([
            button(t!("git.rebase_start").to_string(), "plan:run")
                .button_style(ButtonStyle::Primary)
                .enabled(free),
            button(t!("git.cancel").to_string(), "plan:cancel"),
        ])
        .spacing(6.0),
    );
    scroll(column(rows).spacing(4.0))
}

fn branches_tab(g: &Git) -> Node {
    let free = g.busy.is_none();
    let filter = g.branch_filter.to_lowercase();
    let keep = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
    let branch_item = |prefix: &str, b: &crate::git::history::Branch| -> TreeItem {
        let mut detail = String::new();
        if b.ahead > 0 {
            detail.push_str(&format!("↑{} ", b.ahead));
        }
        if b.behind > 0 {
            detail.push_str(&format!("↓{}", b.behind));
        }
        let label = if b.current {
            format!("● {}", b.name)
        } else {
            b.name.clone()
        };
        tree_item(format!("{prefix}:{}", b.name), label).detail(detail.trim().to_string())
    };
    let local: Vec<TreeItem> = g
        .refs
        .branches
        .iter()
        .filter(|b| !b.remote && keep(&b.name))
        .map(|b| branch_item("b", b))
        .collect();
    let remote: Vec<TreeItem> = g
        .refs
        .branches
        .iter()
        .filter(|b| b.remote && keep(&b.name))
        .map(|b| branch_item("r", b))
        .collect();
    let tags: Vec<TreeItem> = g
        .refs
        .tags
        .iter()
        .filter(|t| keep(t))
        .map(|t| tree_item(format!("t:{t}"), t.clone()))
        .collect();
    let group = |id: &str, label: String, children: Vec<TreeItem>| {
        tree_item(id, label)
            .expandable()
            .expanded(true)
            .children(children)
    };
    let mut refs = tree(
        "refs",
        [
            group("g:local", t!("git.branches_local").to_string(), local),
            group("g:remote", t!("git.branches_remote").to_string(), remote),
            group("g:tags", t!("git.branches_tags").to_string(), tags),
        ],
    );
    if let Some(r) = &g.reference {
        refs = refs.selected(r.clone());
    }
    let mut parts = vec![
        row([
            input(
                "branch_filter",
                g.branch_filter.clone(),
                t!("git.branch_filter").to_string(),
            ),
            tool_button(t!("git.new_branch_here").to_string(), "ref:create", free),
        ])
        .spacing(6.0),
        scroll(refs).height(240.0),
    ];
    let current = g
        .status
        .as_ref()
        .map(|s| s.branch.clone())
        .unwrap_or_default();
    if let Some(id) = &g.reference {
        let (kind, name) = id.split_once(':').unwrap_or(("", id));
        let mut actions = vec![text(name.to_string()).style(TextStyle::Bold)];
        match kind {
            "b" | "r" => {
                let is_current = kind == "b" && name == current;
                actions.extend([
                    tool_button(
                        t!("git.branch_checkout").to_string(),
                        "ref:checkout",
                        free && !is_current,
                    ),
                    tool_button(
                        t!("git.branch_merge", current = current.as_str()).to_string(),
                        "ref:merge",
                        free && !is_current,
                    ),
                    tool_button(
                        t!("git.branch_rebase", current = current.as_str()).to_string(),
                        "ref:rebase",
                        free && !is_current,
                    ),
                    tool_button(t!("git.pull").to_string(), "ref:pull", free),
                    tool_button(t!("git.tab_log").to_string(), "ref:log", true),
                    tool_button(t!("git.compare_local").to_string(), "ref:compare", true),
                ]);
                if kind == "b" {
                    actions.extend([
                        tool_button(t!("git.push").to_string(), "ref:push", free),
                        armed_button(
                            g,
                            t!("git.force_push").to_string(),
                            "confirm:ref:force_push",
                            free,
                        ),
                        tool_button(t!("git.rename").to_string(), "ref:rename", free),
                    ]);
                } else {
                    actions.push(tool_button(t!("git.fetch").to_string(), "ref:fetch", free));
                }
                actions.push(tool_button(
                    t!("git.log_tag_here").to_string(),
                    "ref:tag",
                    free,
                ));
                actions.push(armed_button(
                    g,
                    t!("git.branch_delete").to_string(),
                    "confirm:ref:delete",
                    free && !is_current,
                ));
                if g.notice
                    .as_ref()
                    .is_some_and(|(n, _)| t!("git.branch_not_merged", name = name) == n.as_str())
                {
                    actions.push(armed_button(
                        g,
                        t!("git.branch_force_delete").to_string(),
                        "confirm:ref:force_delete",
                        free,
                    ));
                }
            }
            "t" => actions.extend([
                tool_button(t!("git.checkout").to_string(), "ref:checkout", free),
                tool_button(t!("git.tab_log").to_string(), "ref:log", true),
                tool_button(t!("git.compare_local").to_string(), "ref:compare", true),
                tool_button(t!("git.push_tag").to_string(), "ref:push_tag", free),
                armed_button(
                    g,
                    t!("git.delete_remote_tag").to_string(),
                    "confirm:ref:delete_remote_tag",
                    free,
                ),
                armed_button(g, t!("git.delete").to_string(), "confirm:ref:delete", free),
            ]),
            _ => {}
        }
        parts.push(row(actions).spacing(2.0));
    }
    if let Some(dialog) = &g.ref_dialog {
        let title = match dialog {
            RefDialog::Create => match &g.reference {
                Some(r) => t!(
                    "git.new_branch_from",
                    from = r.split_once(':').map_or(r.as_str(), |x| x.1)
                )
                .to_string(),
                None => t!("git.new_branch_here").to_string(),
            },
            RefDialog::Rename => t!("git.rename").to_string(),
            RefDialog::Tag => t!("git.new_tag_name").to_string(),
        };
        let mut fields = vec![
            text(title).style(TextStyle::Bold),
            input(
                "ref_name",
                g.ref_name.clone(),
                t!("git.new_branch_name").to_string(),
            )
            .on_submit("ref_dialog:ok"),
        ];
        if *dialog == RefDialog::Tag {
            fields.push(input(
                "tag_message",
                g.tag_message.clone(),
                t!("git.new_tag_message").to_string(),
            ));
        }
        fields.push(
            row([
                button(t!("git.ok").to_string(), "ref_dialog:ok")
                    .button_style(ButtonStyle::Primary)
                    .enabled(free),
                button(t!("git.cancel").to_string(), "ref_dialog:cancel"),
            ])
            .spacing(6.0),
        );
        parts.push(column(fields).spacing(4.0));
    }
    column(parts).spacing(6.0)
}

fn tools_tab(g: &Git) -> Node {
    let free = g.busy.is_none();
    let has_repo = g.repo.is_some();
    let mut parts = vec![select(
        "tool",
        Tool::ALL.iter().map(|t| t.label()),
        Some(g.tool.label()),
    )];
    let needs_repo = !matches!(g.tool, Tool::Clone | Tool::Repos);
    if needs_repo && !has_repo {
        parts.push(text(t!("git.no_repo").to_string()).style(TextStyle::Muted));
        return column(parts).spacing(6.0);
    }
    let branches: Vec<String> = g.refs.branches.iter().map(|b| b.name.clone()).collect();
    match g.tool {
        Tool::File => match &g.current_file {
            None => parts.push(text(t!("git.tool_file_none").to_string()).style(TextStyle::Muted)),
            Some(file) => {
                parts.extend([
                    text(file.display().to_string()).style(TextStyle::Mono),
                    row([
                        input(
                            "file_rev",
                            g.file_rev.clone(),
                            t!("git.pick_placeholder").to_string(),
                        )
                        .on_submit("rev:compare"),
                        tool_button(
                            t!("git.file_compare_revision").to_string(),
                            "rev:compare",
                            free,
                        ),
                        armed_button(
                            g,
                            t!("git.file_get_revision").to_string(),
                            "confirm:rev:get",
                            free,
                        ),
                    ])
                    .spacing(6.0),
                    row([
                        tool_button(t!("git.file_history").to_string(), "rev:history", true),
                        tool_button(
                            t!("git.file_history_selection").to_string(),
                            "rev:lines",
                            true,
                        ),
                        tool_button(t!("git.blame").to_string(), "rev:blame", true),
                    ])
                    .spacing(6.0),
                ]);
            }
        },
        Tool::Push => {
            let tags = push_tags();
            parts.extend([
                text(t!("git.push_title", source = g.push.source.as_str()).to_string())
                    .style(TextStyle::Bold),
                row([
                    select(
                        "push_remote",
                        g.remotes.iter().map(|r| r.name.clone()),
                        Some(g.push.remote.clone()),
                    ),
                    input(
                        "push_target",
                        g.push.target.clone(),
                        t!("git.push_target").to_string(),
                    ),
                    checkbox(
                        "push_upstream",
                        t!("git.push_set_upstream").to_string(),
                        g.push.set_upstream,
                    ),
                    text(t!("git.push_tags").to_string()),
                    select(
                        "push_tags",
                        tags.iter().map(|(_, l)| l.clone()),
                        label_of(&tags, &g.push.tags),
                    ),
                ])
                .spacing(6.0),
                row([
                    button(t!("git.push").to_string(), "push:run")
                        .button_style(ButtonStyle::Primary)
                        .enabled(free),
                    armed_button(
                        g,
                        t!("git.force_push").to_string(),
                        "confirm:push:force",
                        free,
                    ),
                ])
                .spacing(6.0),
            ]);
            if g.outgoing.is_empty() {
                parts.push(text(t!("git.push_nothing").to_string()).style(TextStyle::Muted));
            } else {
                let commits: Vec<Node> = g
                    .outgoing
                    .iter()
                    .map(|c| {
                        text(format!("{}  {}", &c.sha[..c.sha.len().min(8)], c.subject))
                            .style(TextStyle::Mono)
                    })
                    .collect();
                parts.push(scroll(column(commits)).height(120.0));
            }
            if g.armed.as_deref() == Some("rejected") {
                parts.push(
                    row([
                        text(t!("git.rejected_title").to_string()).style(TextStyle::Error),
                        tool_button(t!("git.rejected_merge").to_string(), "push:merge", free),
                        tool_button(t!("git.rejected_rebase").to_string(), "push:rebase", free),
                    ])
                    .spacing(6.0),
                );
            }
        }
        Tool::Remotes => {
            if g.remotes.is_empty() {
                parts.push(text(t!("git.no_remote").to_string()).style(TextStyle::Muted));
            }
            let mut remotes = list(
                "remotes",
                g.remotes
                    .iter()
                    .map(|r| item(r.name.clone(), r.name.clone()).detail(r.url.clone())),
            );
            if let Some(r) = &g.remote_edit {
                remotes = remotes.selected(r.clone());
            }
            parts.push(remotes);
            let mut fields = vec![
                input(
                    "remote_name",
                    g.remote_name.clone(),
                    t!("git.remote_name").to_string(),
                ),
                input(
                    "remote_url",
                    g.remote_url.clone(),
                    t!("git.remote_url").to_string(),
                ),
            ];
            if g.remote_edit.is_some() {
                fields.extend([
                    button(t!("git.remote_save").to_string(), "remote:save")
                        .button_style(ButtonStyle::Primary)
                        .enabled(free),
                    armed_button(
                        g,
                        t!("git.delete").to_string(),
                        "confirm:remote:remove",
                        free,
                    ),
                    tool_button(t!("git.remote_add").to_string(), "remote:new", true),
                ]);
            } else {
                fields.push(
                    button(t!("git.remote_add").to_string(), "remote:save")
                        .button_style(ButtonStyle::Primary)
                        .enabled(free),
                );
            }
            parts.push(row(fields).spacing(6.0));
        }
        Tool::Patch => {
            parts.extend([
                row([
                    tool_button(t!("git.patch_local").to_string(), "patch:local", free),
                    tool_button(
                        t!("git.patch_of_commits").to_string(),
                        "patch:commits",
                        free && !g.selected.is_empty(),
                    ),
                    tool_button(
                        t!("git.patch_save").to_string(),
                        "patch:save",
                        !g.patch.is_empty(),
                    ),
                    tool_button(
                        t!("git.patch_copy").to_string(),
                        "patch:copy",
                        !g.patch.is_empty(),
                    ),
                ])
                .spacing(4.0),
                editor("patch_text", g.patch.clone(), "diff")
                    .placeholder(t!("git.patch_paste").to_string())
                    .height(200.0),
                row([
                    text(t!("git.patch_apply").to_string()),
                    tool_button(
                        t!("git.patch_from_text").to_string(),
                        "patch:apply",
                        free && !g.patch.trim().is_empty(),
                    ),
                    tool_button(
                        t!("git.patch_from_file").to_string(),
                        "patch:apply_file",
                        free,
                    ),
                ])
                .spacing(4.0),
            ]);
        }
        Tool::Merge => {
            let mode = match g.merge.mode {
                crate::git::MergeMode::NoFastForward => "no-ff",
                crate::git::MergeMode::FastForwardOnly => "ff-only",
                crate::git::MergeMode::Squash => "squash",
                crate::git::MergeMode::Default => "default",
            };
            parts.extend([
                row([
                    select(
                        "merge_branch",
                        branches.clone(),
                        (!g.merge_branch.is_empty()).then(|| g.merge_branch.clone()),
                    )
                    .placeholder(t!("git.branch_pick").to_string()),
                    select(
                        "merge_mode",
                        MERGE_MODES.iter().map(|(_, l)| l()),
                        MERGE_MODES
                            .iter()
                            .find(|(m, _)| *m == mode)
                            .map(|(_, l)| l()),
                    ),
                ])
                .spacing(6.0),
                row([
                    checkbox(
                        "merge_no_commit",
                        t!("git.merge_no_commit").to_string(),
                        g.merge.no_commit,
                    ),
                    checkbox(
                        "merge_no_verify",
                        t!("git.no_verify").to_string(),
                        g.merge.no_verify,
                    ),
                    checkbox(
                        "merge_unrelated",
                        t!("git.merge_unrelated").to_string(),
                        g.merge.allow_unrelated,
                    ),
                ])
                .spacing(8.0),
                input(
                    "merge_message",
                    g.merge.message.clone(),
                    t!("git.merge_message").to_string(),
                ),
                button(t!("git.merge").to_string(), "merge:run")
                    .button_style(ButtonStyle::Primary)
                    .enabled(free && !g.merge_branch.is_empty()),
            ]);
        }
        Tool::Rebase => {
            let o = &g.rebase;
            parts.extend([
                row([
                    select(
                        "rebase_branch",
                        branches,
                        (!g.rebase_branch.is_empty()).then(|| g.rebase_branch.clone()),
                    )
                    .placeholder(t!("git.branch_pick").to_string()),
                    input(
                        "rebase_onto",
                        o.onto.clone(),
                        t!("git.rebase_onto_field").to_string(),
                    ),
                ])
                .spacing(6.0),
                row([
                    checkbox(
                        "rebase_merges",
                        t!("git.rebase_merges").to_string(),
                        o.rebase_merges,
                    ),
                    checkbox(
                        "rebase_keep_empty",
                        t!("git.rebase_keep_empty").to_string(),
                        o.keep_empty,
                    ),
                    checkbox("rebase_root", t!("git.rebase_root").to_string(), o.root),
                    checkbox(
                        "rebase_update_refs",
                        t!("git.rebase_update_refs").to_string(),
                        o.update_refs,
                    ),
                    checkbox(
                        "rebase_autosquash",
                        t!("git.rebase_autosquash").to_string(),
                        o.autosquash,
                    ),
                ])
                .spacing(8.0),
                button(t!("git.rebase_start").to_string(), "rebase:run")
                    .button_style(ButtonStyle::Primary)
                    .enabled(free && (!g.rebase_branch.is_empty() || o.root)),
            ]);
        }
        Tool::Clean => {
            parts.push(
                row([
                    checkbox(
                        "clean_ignored",
                        t!("git.clean_ignored").to_string(),
                        g.clean_ignored,
                    ),
                    tool_button(t!("git.clean_action").to_string(), "clean:preview", free),
                ])
                .spacing(6.0),
            );
            if !g.clean.is_empty() {
                parts.push(text(t!("git.clean_text", n = g.clean.len()).to_string()));
                let shown: Vec<Node> = g
                    .clean
                    .iter()
                    .take(50)
                    .map(|f| text(f.clone()).style(TextStyle::Mono))
                    .collect();
                parts.push(scroll(column(shown)).height(140.0));
                if g.clean.len() > 50 {
                    parts.push(text(
                        t!("git.clean_more", n = g.clean.len() - 50).to_string(),
                    ));
                }
                parts.push(armed_button(
                    g,
                    t!("git.clean_run").to_string(),
                    "confirm:clean:run",
                    free,
                ));
            }
        }
        Tool::Trees => {
            parts.push(
                row([
                    text(t!("git.submodules").to_string()).style(TextStyle::Heading),
                    tool_button(
                        t!("git.submodules_update").to_string(),
                        "trees:update",
                        free,
                    ),
                ])
                .spacing(6.0),
            );
            if g.submodules.is_empty() {
                parts.push(text(t!("git.no_submodules").to_string()).style(TextStyle::Muted));
            }
            for s in &g.submodules {
                let state = match s.state {
                    '-' => t!("git.submodule_missing").to_string(),
                    '+' => t!("git.submodule_moved").to_string(),
                    'U' => t!("git.submodule_conflict").to_string(),
                    _ => String::new(),
                };
                parts.push(
                    text(format!(
                        "{}  {}  {state}",
                        s.path,
                        &s.sha[..s.sha.len().min(8)]
                    ))
                    .style(TextStyle::Mono),
                );
            }
            parts.push(text(t!("git.worktrees").to_string()).style(TextStyle::Heading));
            parts.push(list(
                "worktrees",
                g.worktrees.iter().map(|w| {
                    item(w.path.display().to_string(), w.path.display().to_string())
                        .detail(w.branch.clone())
                }),
            ));
        }
        Tool::Clone => {
            parts.extend([
                input(
                    "clone_url",
                    g.clone_url.clone(),
                    t!("git.clone_url").to_string(),
                ),
                input(
                    "clone_dest",
                    g.clone_dest.clone(),
                    t!("git.clone_dest").to_string(),
                ),
                button(t!("git.clone").to_string(), "clone:run")
                    .button_style(ButtonStyle::Primary)
                    .enabled(free),
            ]);
        }
        Tool::Repos => {
            if g.repos.is_empty() {
                parts.push(text(t!("git.no_repos").to_string()).style(TextStyle::Muted));
            } else {
                parts.push(
                    row([
                        tool_button(t!("git.fetch").to_string(), "repos:fetch", free),
                        tool_button(t!("git.pull").to_string(), "repos:pull", free),
                        tool_button(t!("git.push").to_string(), "repos:push", free),
                    ])
                    .spacing(4.0),
                );
                let mut repos = list(
                    "repos",
                    g.repos.iter().map(|r| {
                        let name = r
                            .root
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        item(r.root.display().to_string(), name).detail(format!(
                            "{} · {}",
                            state::branch_label(&r.branch, r.ahead, r.behind),
                            t!("git.repo_changes", n = r.changes)
                        ))
                    }),
                );
                if let Some(repo) = &g.repo {
                    repos = repos.selected(repo.display().to_string());
                }
                parts.push(repos);
            }
        }
    }
    column(parts).spacing(6.0)
}

/// Preferences › Plugins › Git.
pub fn settings(s: &Settings) -> Node {
    let pull = PULL_MODES
        .iter()
        .find(|(m, _)| *m == s.pull_mode)
        .map(|(_, l)| l());
    let merge = MERGE_MODES
        .iter()
        .find(|(m, _)| *m == s.merge_mode)
        .map(|(_, l)| l());
    column([
        text(t!("git.settings_pull").to_string()).style(TextStyle::Bold),
        select("pull_mode", PULL_MODES.iter().map(|(_, l)| l()), pull),
        text(t!("git.settings_merge").to_string()).style(TextStyle::Bold),
        select("merge_mode", MERGE_MODES.iter().map(|(_, l)| l()), merge),
        text(t!("git.settings_auto_fetch").to_string()).style(TextStyle::Bold),
        input("auto_fetch", s.auto_fetch_minutes.to_string(), "0"),
        text(t!("git.settings_auto_fetch_hint").to_string()).style(TextStyle::Muted),
    ])
    .spacing(6.0)
    .padding(8.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_graph_shows_the_commit_on_its_lane_and_the_lanes_going_on() {
        let row = crate::git::graph::GraphRow {
            lane: 1,
            color: 0,
            through: vec![(0, 0)],
            from_above: vec![],
            to_below: vec![(1, 2)],
        };
        assert_eq!(graph_text(&row, 3, true), "│●│");
        assert_eq!(graph_text(&row, 3, false), "│○│");
    }

    #[test]
    fn every_tool_has_a_name_and_the_panel_draws_without_a_repository() {
        for tool in Tool::ALL {
            assert!(!tool.label().is_empty());
        }
        let mut g = Git::new();
        g.searched = true;
        let drawn = serde_json::to_string(&panel(&g)).unwrap();
        assert!(drawn.contains("\"init\""));
        for tool in Tool::ALL {
            g.tab = Tab::Tools;
            g.tool = tool;
            let _ = panel(&g);
        }
    }
}
