//! What the buttons, lists and fields of the panel, the commands and the
//! buttons of the diff tabs do.

use std::future::Future;
use std::path::PathBuf;

use simpleedit_plugin::protocol::TreeAction;
use simpleedit_plugin::protocol::{Context, DiffAction, DiffActionParams, UiEvent};
use simpleedit_plugin::Host;

use crate::git::{self, history, tools, Change, Source};
use crate::state::{Dialog, DiffTab, RefDialog, RepoLine, Tab, Tool};
use crate::{relative, source_of, Ctx, GitPlugin, PANEL};

/// What a git call that answers nothing says once done.
fn ok(result: Result<(), String>, said: impl Into<String>) -> Result<String, String> {
    result.map(|()| said.into())
}

/// Runs a git operation on the repository shown, in the background.
fn act<F>(p: &GitPlugin, ctx: &Ctx, label: String, op: impl FnOnce(PathBuf) -> F)
where
    F: Future<Output = Result<String, String>> + Send + 'static,
{
    let Some(repo) = ctx.repo() else {
        return;
    };
    let fut = op(repo);
    let ctx = ctx.clone();
    p.spawn(async move { ctx.act(label, fut).await });
}

fn refresh(p: &GitPlugin, ctx: &Ctx) {
    let ctx = ctx.clone();
    p.spawn(async move { ctx.refresh().await });
}

pub fn command(p: &GitPlugin, ctx: &Ctx, host: &Host, command: &str, context: &Context) {
    let show = |tab: Tab| {
        ctx.with(|g| g.tab = tab);
        host.show_panel(PANEL);
        ctx.render();
    };
    match command {
        "git.show" => show(Tab::Commit),
        "git.commit" => show(Tab::Commit),
        "git.branches" => show(Tab::Branches),
        "git.log" => {
            ctx.with(|g| g.log_path = None);
            show(Tab::Log);
            refresh(p, ctx);
        }
        "git.stash" => show(Tab::Stash),
        "git.push" => push_current(p, ctx),
        "git.pull" => {
            let mode = ctx.with(|g| g.settings.pull());
            act(p, ctx, t!("git.pull").to_string(), move |repo| async move {
                ok(git::pull(&repo, mode, None).await, t!("git.pulled"))
            });
        }
        "git.fetch" => act(p, ctx, t!("git.fetch").to_string(), |repo| async move {
            ok(git::fetch(&repo).await, t!("git.fetched"))
        }),
        "git.blame" => p.blame(),
        "git.blame_close" => {
            if let Some(blame) = ctx.with(|g| g.blame.take()) {
                host.annotate(&blame.path, None);
            }
        }
        "git.blame_blanks" | "git.blame_moves" | "git.blame_across" | "git.blame_show_hidden" => {
            ctx.with(|g| {
                let o = &mut g.blame_options;
                match command {
                    "git.blame_blanks" => o.ignore_blanks = !o.ignore_blanks,
                    "git.blame_moves" => o.moves_in_file = !o.moves_in_file,
                    "git.blame_across" => o.moves_across = !o.moves_across,
                    _ => o.hidden.clear(),
                }
            });
            p.blame();
        }
        "git.file_history" => {
            let Some(file) = context.file.clone() else {
                return;
            };
            let ctx = ctx.clone();
            let host = host.clone();
            p.spawn(async move {
                let Some(root) = git::repo_root(&file).await else {
                    host.error(t!("git.file_no_repo"));
                    return;
                };
                ctx.with(|g| {
                    g.repo = Some(root.clone());
                    g.log_path = relative(&root, &file);
                    g.log_rev = "HEAD".into();
                    g.tab = Tab::Log;
                    g.selected.clear();
                });
                host.show_panel(PANEL);
                ctx.refresh().await;
            });
        }
        "git.file_diff" => {
            let Some(file) = context.file.clone() else {
                return;
            };
            let ctx = ctx.clone();
            let host = host.clone();
            p.spawn(async move {
                let Some(root) = git::repo_root(&file).await else {
                    host.error(t!("git.file_no_repo"));
                    return;
                };
                let Some(path) = relative(&root, &file) else {
                    return;
                };
                let change = Change {
                    path: path.clone(),
                    from: None,
                    kind: git::Kind::Modified,
                };
                let tab = DiffTab {
                    title: t!("git.diff_with_tree", name = "HEAD").to_string(),
                    files: vec![(Source::Tree("HEAD".into()), change)],
                    options: Default::default(),
                    start: 0,
                };
                ctx.with(|g| g.repo = Some(root));
                ctx.show_diffs(format!("file:{path}"), tab).await;
            });
        }
        "git.file_compare" | "git.file_get" => {
            ctx.with(|g| {
                g.tab = Tab::Tools;
                g.tool = Tool::File;
            });
            host.show_panel(PANEL);
            ctx.render();
        }
        "git.file_history_selection" => {
            let ctx = ctx.clone();
            let host = host.clone();
            p.spawn(async move {
                let Ok(file) = host.active_file() else {
                    return;
                };
                let (Some(path), Some((first, last))) = (file.path, file.lines) else {
                    return;
                };
                let Some(root) = git::repo_root(&path).await else {
                    host.error(t!("git.file_no_repo"));
                    return;
                };
                let Some(inner) = relative(&root, &path) else {
                    return;
                };
                ctx.with(|g| {
                    g.repo = Some(root);
                    g.log_path = None;
                    g.filter.lines = Some((inner, first, last));
                    g.show_filters = true;
                    g.log_rev = "HEAD".into();
                    g.tab = Tab::Log;
                    g.selected.clear();
                });
                host.show_panel(PANEL);
                ctx.refresh().await;
            });
        }
        "git.blame_hide" => {
            let ctx = ctx.clone();
            p.spawn(async move {
                let Ok(file) = ctx.host.active_file() else {
                    return;
                };
                let line = file.lines.map_or(0, |(first, _)| first.saturating_sub(1));
                let hidden = ctx.with(|g| {
                    let sha = g.blame.as_ref()?.shas.get(line)?.clone();
                    if sha.chars().all(|c| c == '0') {
                        return None;
                    }
                    g.blame_options.hidden.push(sha);
                    Some(())
                });
                if hidden.is_some() {
                    crate::blame_now(&ctx).await;
                }
            });
        }
        "git.blame_previous" => {
            let ctx = ctx.clone();
            let host = host.clone();
            p.spawn(async move { blame_previous(&ctx, &host).await });
        }
        "git.init" => init(p, ctx),
        "git.push_options" => {
            ctx.with(|g| {
                g.tab = Tab::Tools;
                g.tool = Tool::Push;
            });
            host.show_panel(PANEL);
            ctx.render();
            tool_opened(p, ctx);
        }
        "git.clone" => {
            ctx.with(|g| {
                g.tab = Tab::Tools;
                g.tool = Tool::Clone;
            });
            host.show_panel(PANEL);
            ctx.render();
        }
        _ => {}
    }
}

fn init(p: &GitPlugin, ctx: &Ctx) {
    let Some(dir) = ctx.with(|g| g.init_dir.clone()) else {
        return;
    };
    let ctx2 = ctx.clone();
    p.spawn(async move {
        let result = git::run(&dir, &["init"]).await.map(|_| String::new());
        ctx2.with(|g| {
            if result.is_ok() {
                g.repo = Some(dir.clone());
            }
            g.say(result);
        });
        ctx2.refresh().await;
    });
}

/// Pushes the current branch (its upstream made the first time).
fn push_current(p: &GitPlugin, ctx: &Ctx) {
    let Some((branch, upstream)) = ctx.with(|g| {
        g.status
            .as_ref()
            .map(|s| (s.branch.clone(), s.upstream.is_some()))
    }) else {
        return;
    };
    if branch.is_empty() || branch == "HEAD" {
        ctx.with(|g| g.say(Err(t!("git.err_detached_push").to_string())));
        ctx.render();
        return;
    }
    act(p, ctx, t!("git.push").to_string(), move |repo| async move {
        ok(git::push(&repo, &branch, upstream).await, t!("git.pushed"))
    });
}

/// The settings panel (Preferences › Plugins).
pub fn settings_event(ctx: &Ctx, event: &UiEvent) {
    let text = event.text().unwrap_or_default().to_string();
    ctx.with(|g| match event.action.as_str() {
        "pull_mode" => {
            if let Some(mode) = crate::ui::PULL_MODES.iter().find(|(_, l)| l() == text) {
                g.settings.pull_mode = mode.0.to_string();
            }
        }
        "merge_mode" => {
            if let Some(mode) = crate::ui::MERGE_MODES.iter().find(|(_, l)| l() == text) {
                g.settings.merge_mode = mode.0.to_string();
            }
        }
        "auto_fetch" => {
            g.settings.auto_fetch_minutes = text.trim().parse().unwrap_or(0).min(24 * 60);
        }
        _ => {}
    });
    ctx.save_settings();
    ctx.render_settings();
}

/// A file of the Commit tab shown in a diff tab, with the others of its list.
fn show_change(p: &GitPlugin, ctx: &Ctx, staged: bool, path: &str) {
    let tab = ctx.with(|g| {
        let status = g.status.as_ref()?;
        let list = if staged {
            &status.staged
        } else {
            &status.unstaged
        };
        let start = list.iter().position(|c| c.path == path)?;
        Some(DiffTab {
            title: if staged {
                t!("git.diff_staged").to_string()
            } else {
                t!("git.diff_changes").to_string()
            },
            files: list
                .iter()
                .map(|c| (source_of(false, c, staged), c.clone()))
                .collect(),
            options: Default::default(),
            start,
        })
    });
    let Some(tab) = tab else {
        return;
    };
    let id = if staged {
        "changes:staged"
    } else {
        "changes:unstaged"
    };
    let ctx = ctx.clone();
    p.spawn(async move { ctx.show_diffs(id.to_string(), tab).await });
}

/// The files of a commit (or a stash, or between two revisions) in a diff tab.
fn show_files(
    p: &GitPlugin,
    ctx: &Ctx,
    id: String,
    title: String,
    files: Vec<(Source, Change)>,
    start: usize,
) {
    let tab = DiffTab {
        title,
        files,
        options: Default::default(),
        start,
    };
    let ctx = ctx.clone();
    p.spawn(async move { ctx.show_diffs(id, tab).await });
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

pub fn event(p: &GitPlugin, ctx: &Ctx, host: &Host, event: &UiEvent) {
    let action = event.action.as_str();
    let text = event.text().unwrap_or_default().to_string();
    let checked = event.checked().unwrap_or(false);
    // A click elsewhere disarms a button that asks to be clicked twice.
    if !action.starts_with("confirm:") {
        ctx.with(|g| g.armed = None);
    }
    match action {
        // ── The panel ──────────────────────────────────────────────────
        "tab:commit" | "tab:stash" | "tab:log" | "tab:branches" | "tab:tools" => {
            ctx.with(|g| {
                g.tab = match action {
                    "tab:stash" => Tab::Stash,
                    "tab:log" => Tab::Log,
                    "tab:branches" => Tab::Branches,
                    "tab:tools" => Tab::Tools,
                    _ => Tab::Commit,
                }
            });
            ctx.render();
            if action == "tab:tools" {
                tool_opened(p, ctx);
            }
        }
        "refresh" => refresh(p, ctx),
        "dismiss" => {
            ctx.with(|g| g.notice = None);
            ctx.render();
        }
        "init" => init(p, ctx),
        "fetch" | "pull" | "push" => {
            command(p, ctx, host, &format!("git.{action}"), &Context::default())
        }
        "confirm:undo_commit" => {
            if !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            act(
                p,
                ctx,
                t!("git.undo_commit").to_string(),
                |repo| async move { git::undo_last_commit(&repo).await },
            );
        }

        // ── Commit: the files ──────────────────────────────────────────
        "staged" | "unstaged" => {
            let staged = action == "staged";
            ctx.with(|g| g.picked = Some((staged, text.clone())));
            ctx.render();
            show_change(p, ctx, staged, &text);
        }
        "stage_all" => {
            let paths = ctx.with(|g| {
                g.status.as_ref().map_or_else(Vec::new, |s| {
                    s.unstaged
                        .iter()
                        .filter(|c| c.kind != git::Kind::Conflict)
                        .map(|c| c.path.clone())
                        .collect::<Vec<_>>()
                })
            });
            act(
                p,
                ctx,
                t!("git.stage").to_string(),
                move |repo| async move { ok(git::stage(&repo, &paths).await, "") },
            );
        }
        "unstage_all" => {
            let paths = ctx.with(|g| {
                g.status.as_ref().map_or_else(Vec::new, |s| {
                    s.staged.iter().map(|c| c.path.clone()).collect::<Vec<_>>()
                })
            });
            act(
                p,
                ctx,
                t!("git.unstage").to_string(),
                move |repo| async move { ok(git::unstage(&repo, &paths).await, "") },
            );
        }
        "file:stage"
        | "file:unstage"
        | "confirm:file:rollback"
        | "confirm:file:delete"
        | "file:stash"
        | "file:ignore"
        | "file:open"
        | "file:diff"
        | "file:history"
        | "file:commit_only" => {
            let Some((staged, path)) = ctx.with(|g| g.picked.clone()) else {
                return;
            };
            let Some(change) = ctx.with(|g| g.change(staged, &path).cloned()) else {
                return;
            };
            file_action(p, ctx, host, action, staged, change);
        }

        // ── Commit: the message ────────────────────────────────────────
        "message" => ctx.with(|g| g.message = text),
        "amend" => {
            ctx.with(|g| g.amend = checked);
            if checked && ctx.with(|g| g.message.trim().is_empty()) {
                let ctx = ctx.clone();
                p.spawn(async move {
                    let Some(repo) = ctx.repo() else {
                        return;
                    };
                    if let Ok(message) = git::last_message(&repo).await {
                        ctx.with(|g| g.message = message);
                    }
                    ctx.render();
                });
            } else {
                ctx.render();
            }
        }
        "show_options" => {
            ctx.with(|g| g.show_options = !g.show_options);
            ctx.render();
        }
        "signoff" => {
            ctx.with(|g| g.options.signoff = checked);
            ctx.render();
        }
        "no_verify" => {
            ctx.with(|g| g.options.no_verify = checked);
            ctx.render();
        }
        "author" => ctx.with(|g| g.options.author = text),
        "recent" => {
            ctx.with(|g| {
                if let Some(m) = g.recent.iter().find(|m| first_line(m) == text) {
                    g.message = m.clone();
                }
            });
            ctx.render();
        }
        "commit" | "commit_push" => commit(p, ctx, action == "commit_push", None),

        // ── Conflicts ──────────────────────────────────────────────────
        "conflicts" => {
            ctx.with(|g| g.picked = Some((false, text)));
            ctx.render();
        }
        "accept_yours" | "accept_theirs" | "merge_file" => {
            let Some((_, path)) = ctx.with(|g| g.picked.clone()) else {
                return;
            };
            if action == "merge_file" {
                let ctx = ctx.clone();
                let host = host.clone();
                p.spawn(async move {
                    let Some(repo) = ctx.repo() else {
                        return;
                    };
                    // As git rebuilds it (with its base), unless the file on
                    // disk was worked on: that work is kept.
                    let rebuilt = git::merge_text(&repo, &path).await;
                    let disk = tokio::fs::read_to_string(repo.join(&path)).await.ok();
                    let rebasing = git::operation(&repo).await == Some(git::Operation::Rebase);
                    match git::conflicts::choose_text(rebuilt.as_deref(), disk.as_deref(), rebasing)
                    {
                        Ok(text) => {
                            let tab = format!("merge:{path}");
                            if let Err(e) = host.open_merge(&tab, repo.join(&path), text, rebasing)
                            {
                                host.error(e.message);
                            }
                        }
                        Err(git::conflicts::Unavailable::Settled) => {
                            host.notify(t!("git.already_resolved"))
                        }
                        Err(git::conflicts::Unavailable::NotMergeable) => {
                            host.error(t!("git.not_mergeable"))
                        }
                    }
                });
                return;
            }
            let yours = action == "accept_yours";
            let rebasing = ctx.with(|g| {
                g.status.as_ref().and_then(|s| s.operation) == Some(git::Operation::Rebase)
            });
            act(
                p,
                ctx,
                t!("git.resolve").to_string(),
                move |repo| async move { ok(git::accept(&repo, &path, yours, rebasing).await, "") },
            );
        }
        "op:continue" | "op:skip" | "confirm:op:abort" => {
            let Some(operation) = ctx.with(|g| g.status.as_ref().and_then(|s| s.operation)) else {
                return;
            };
            if action == "confirm:op:abort" && !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            let label = match action {
                "op:continue" => t!("git.continue"),
                "op:skip" => t!("git.skip"),
                _ => t!("git.abort"),
            }
            .to_string();
            let action = action.to_string();
            act(p, ctx, label, move |repo| async move {
                let result = match action.as_str() {
                    "op:continue" => git::continue_operation(&repo, operation).await,
                    "op:skip" => git::skip_operation(&repo, operation).await,
                    _ => git::abort_operation(&repo, operation).await,
                };
                ok(result, "")
            });
        }

        // ── Stash ──────────────────────────────────────────────────────
        "stashes" => {
            let index: Option<usize> = text.parse().ok();
            ctx.with(|g| {
                g.stash = index;
                g.stash_files.clear();
            });
            ctx.render();
            if let Some(index) = index {
                let ctx = ctx.clone();
                p.spawn(async move {
                    let Some(repo) = ctx.repo() else {
                        return;
                    };
                    let files = history::stash_files(&repo, index).await.unwrap_or_default();
                    ctx.with(|g| {
                        if g.stash == Some(index) {
                            g.stash_files = files;
                        }
                    });
                    ctx.render();
                });
            }
        }
        "stash_files" => {
            let (Some(index), files) = ctx.with(|g| (g.stash, g.stash_files.clone())) else {
                return;
            };
            let start = files.iter().position(|c| c.path == text).unwrap_or(0);
            let files = files
                .into_iter()
                .map(|c| (Source::Stash(index), c))
                .collect();
            show_files(
                p,
                ctx,
                format!("stash:{index}"),
                format!("stash@{{{index}}}"),
                files,
                start,
            );
        }
        "stash_message" => ctx.with(|g| g.stash_message = text),
        "stash_branch" => ctx.with(|g| g.stash_branch = text),
        "stash_keep_index" => {
            ctx.with(|g| g.stash_keep_index = checked);
            ctx.render();
        }
        "stash_staged" => {
            ctx.with(|g| g.stash_staged = checked);
            ctx.render();
        }
        "stash:push" => {
            let (message, keep, staged) = ctx.with(|g| {
                (
                    std::mem::take(&mut g.stash_message),
                    g.stash_keep_index,
                    g.stash_staged,
                )
            });
            act(
                p,
                ctx,
                t!("git.stash_push").to_string(),
                move |repo| async move {
                    ok(history::stash_push(&repo, &message, keep, staged).await, "")
                },
            );
        }
        "stash:apply" | "stash:pop" | "confirm:stash:drop" | "stash:branch" => {
            let Some(index) = ctx.with(|g| g.stash) else {
                return;
            };
            if action == "confirm:stash:drop" && !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            let name = ctx.with(|g| g.stash_branch.clone());
            let action = action.to_string();
            act(
                p,
                ctx,
                t!("git.tab_stash").to_string(),
                move |repo| async move {
                    let result = match action.as_str() {
                        "stash:apply" => history::stash_apply(&repo, index).await,
                        "stash:pop" => history::stash_pop(&repo, index).await,
                        "stash:branch" => history::stash_branch(&repo, &name, index).await,
                        _ => history::stash_drop(&repo, index).await,
                    };
                    ok(result, "")
                },
            );
        }

        // ── Log ────────────────────────────────────────────────────────
        "search" => ctx.with(|g| g.search = text),
        "search_run" => {
            ctx.with(|g| g.limit = crate::state::PAGE);
            refresh(p, ctx);
        }
        "search_mode" => {
            ctx.with(|g| {
                if let Some((mode, _)) = crate::ui::search_modes()
                    .into_iter()
                    .find(|(_, l)| *l == text)
                {
                    g.search_mode = mode;
                }
            });
            refresh(p, ctx);
        }
        "log_rev" => {
            ctx.with(|g| {
                g.log_rev = if text == crate::ui::all_branches() {
                    history::ALL.into()
                } else if text == t!("git.reflog") {
                    history::REFLOG.into()
                } else if text == t!("git.head_current") {
                    "HEAD".into()
                } else {
                    text
                };
                g.limit = crate::state::PAGE;
                g.selected.clear();
            });
            refresh(p, ctx);
        }
        "log:clear_file" => {
            ctx.with(|g| g.log_path = None);
            refresh(p, ctx);
        }
        "load_more" => {
            ctx.with(|g| g.limit += crate::state::PAGE);
            refresh(p, ctx);
        }
        "several" => {
            ctx.with(|g| {
                g.several = checked;
                if !checked {
                    let last = g.selected.pop();
                    g.selected = last.into_iter().collect();
                }
                g.dialog = None;
            });
            ctx.render();
        }
        "commits" => {
            if ctx.with(|g| g.several) {
                ctx.with(|g| {
                    if let Some(i) = g.selected.iter().position(|s| *s == text) {
                        g.selected.remove(i);
                    } else {
                        g.selected.push(text.clone());
                    }
                });
            } else {
                ctx.with(|g| g.selected = vec![text.clone()]);
            }
            ctx.with(|g| g.dialog = None);
            let ctx = ctx.clone();
            p.spawn(async move { ctx.pick_commit(text).await });
        }
        "commit_files" => {
            let (commit, files) = ctx.with(|g| (g.commit().cloned(), g.commit_files.clone()));
            let Some(commit) = commit else {
                return;
            };
            let start = files.iter().position(|c| c.path == text).unwrap_or(0);
            ctx.with(|g| g.commit_file = Some(text.clone()));
            ctx.render();
            let files = files
                .into_iter()
                .map(|c| (Source::Commit(commit.sha.clone()), c))
                .collect();
            show_files(
                p,
                ctx,
                format!("commit:{}", commit.sha),
                format!("{} {}", short(&commit.sha), commit.subject),
                files,
                start,
            );
        }
        "show_filters" => {
            ctx.with(|g| g.show_filters = !g.show_filters);
            ctx.render();
        }
        "filter_author" => ctx.with(|g| g.filter.author = text),
        "filter_since" => ctx.with(|g| g.filter.since = text),
        "filter_until" => ctx.with(|g| g.filter.until = text),
        "filter_path" => ctx.with(|g| g.filter.path = text),
        "filter_first_parent" | "filter_no_merges" => {
            ctx.with(|g| {
                if action == "filter_first_parent" {
                    g.filter.first_parent = checked;
                } else {
                    g.filter.no_merges = checked;
                }
            });
            refresh(p, ctx);
        }
        "filter:apply" => {
            ctx.with(|g| g.limit = crate::state::PAGE);
            refresh(p, ctx);
        }
        "filter:clear" => {
            ctx.with(|g| g.filter = Default::default());
            refresh(p, ctx);
        }
        "cfile:get" | "cfile:apply" | "confirm:cfile:remove" | "cfile:extract" => {
            let (commit, file) =
                ctx.with(|g| (g.commit().map(|c| c.sha.clone()), g.commit_file.clone()));
            let (Some(sha), Some(file)) = (commit, file) else {
                return;
            };
            if action == "cfile:extract" {
                ctx.with(|g| {
                    g.dialog = Some(Dialog::Extract);
                    g.dialog_text.clear();
                });
                ctx.render();
                return;
            }
            if action == "confirm:cfile:remove" && !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            let action = action.to_string();
            act(
                p,
                ctx,
                t!("git.tab_log").to_string(),
                move |repo| async move {
                    match action.as_str() {
                        "cfile:get" => ok(
                            tools::get_from_revision(&repo, &sha, &file).await,
                            t!("git.file_got"),
                        ),
                        "cfile:apply" => ok(
                            tools::apply_from_commit(&repo, &sha, &[file]).await,
                            t!("git.patch_applied"),
                        ),
                        _ => ok(tools::remove_from_commit(&repo, &sha, &[file]).await, ""),
                    }
                },
            );
        }
        a if a.starts_with("log:") || a.starts_with("confirm:log:") => log_action(p, ctx, host, a),
        "dialog_text" => ctx.with(|g| g.dialog_text = text),
        "reset_mode" => {
            ctx.with(|g| {
                if let Some((mode, _)) = crate::ui::reset_modes()
                    .into_iter()
                    .find(|(_, l)| *l == text)
                {
                    g.reset_mode = mode.to_string();
                }
            });
            ctx.render();
        }
        "dialog:cancel" => {
            ctx.with(|g| g.dialog = None);
            ctx.render();
        }
        "dialog:ok" => dialog_ok(p, ctx),
        a if a.starts_with("plan_action:") => {
            let i: usize = a["plan_action:".len()..].parse().unwrap_or(usize::MAX);
            ctx.with(|g| {
                if let Some((_, plan)) = &mut g.plan {
                    if let (Some(item), Some((action, _))) = (
                        plan.get_mut(i),
                        crate::ui::plan_actions()
                            .into_iter()
                            .find(|(_, l)| *l == text),
                    ) {
                        item.action = action;
                    }
                }
            });
            ctx.render();
        }
        a if a.starts_with("plan_up:") || a.starts_with("plan_down:") => {
            let up = a.starts_with("plan_up:");
            let i: usize = a
                .split(':')
                .nth(1)
                .and_then(|n| n.parse().ok())
                .unwrap_or(usize::MAX);
            ctx.with(|g| {
                if let Some((_, plan)) = &mut g.plan {
                    if up && i > 0 && i < plan.len() {
                        plan.swap(i, i - 1);
                    } else if !up && i + 1 < plan.len() {
                        plan.swap(i, i + 1);
                    }
                }
            });
            ctx.render();
        }
        a if a.starts_with("plan_subject:") => {
            let i: usize = a["plan_subject:".len()..].parse().unwrap_or(usize::MAX);
            ctx.with(|g| {
                if let Some((_, plan)) = &mut g.plan {
                    if let Some(item) = plan.get_mut(i) {
                        item.message = text;
                    }
                }
            });
        }
        "plan:cancel" => {
            ctx.with(|g| g.plan = None);
            ctx.render();
        }
        "plan:run" => {
            let Some((base, plan)) = ctx.with(|g| g.plan.take()) else {
                return;
            };
            act(
                p,
                ctx,
                t!("git.rebase").to_string(),
                move |repo| async move {
                    ok(
                        history::interactive_rebase(&repo, base.as_deref(), &plan).await,
                        "",
                    )
                },
            );
        }

        // ── Branches ───────────────────────────────────────────────────
        "refs" => {
            if let Some((item, how)) = event.tree_item() {
                if how == TreeAction::Click {
                    ctx.with(|g| {
                        g.reference = Some(item.to_string());
                        g.ref_dialog = None;
                    });
                    ctx.render();
                }
            }
        }
        "branch_filter" => {
            ctx.with(|g| g.branch_filter = text);
            ctx.render();
        }
        "ref_name" => ctx.with(|g| g.ref_name = text),
        "tag_message" => ctx.with(|g| g.tag_message = text),
        "ref:create" | "ref:rename" | "ref:tag" => {
            ctx.with(|g| {
                g.ref_dialog = Some(match action {
                    "ref:create" => RefDialog::Create,
                    "ref:rename" => RefDialog::Rename,
                    _ => RefDialog::Tag,
                });
                g.ref_name = if action == "ref:rename" {
                    g.reference
                        .as_deref()
                        .map(ref_name)
                        .unwrap_or_default()
                        .to_string()
                } else {
                    String::new()
                };
                g.tag_message.clear();
            });
            ctx.render();
        }
        "ref_dialog:cancel" => {
            ctx.with(|g| g.ref_dialog = None);
            ctx.render();
        }
        "ref_dialog:ok" => ref_dialog_ok(p, ctx),
        a if a.starts_with("ref:") || a.starts_with("confirm:ref:") => ref_action(p, ctx, a),

        // ── Tools ──────────────────────────────────────────────────────
        "tool" => {
            ctx.with(|g| {
                if let Some(tool) = Tool::ALL.into_iter().find(|t| t.label() == text) {
                    g.tool = tool;
                }
            });
            ctx.render();
            tool_opened(p, ctx);
        }
        a => tool_event(p, ctx, host, a, text, checked),
    }
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or_default()
}

fn ref_name(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, n)| n)
}

fn file_action(p: &GitPlugin, ctx: &Ctx, host: &Host, action: &str, staged: bool, change: Change) {
    let path = change.path.clone();
    match action {
        "file:stage" => act(
            p,
            ctx,
            t!("git.stage").to_string(),
            move |repo| async move { ok(git::stage(&repo, &[path]).await, t!("git.file_staged")) },
        ),
        "file:unstage" => act(
            p,
            ctx,
            t!("git.unstage").to_string(),
            move |repo| async move { ok(git::unstage(&repo, &[path]).await, t!("git.file_unstaged")) },
        ),
        "confirm:file:rollback" => {
            if !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            act(
                p,
                ctx,
                t!("git.rollback").to_string(),
                move |repo| async move { ok(git::rollback(&repo, &change, staged).await, "") },
            );
        }
        "confirm:file:delete" => {
            if !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            act(
                p,
                ctx,
                t!("git.delete").to_string(),
                move |repo| async move { ok(git::delete(&repo, &change).await, "") },
            );
        }
        "file:stash" => act(
            p,
            ctx,
            t!("git.stash_file").to_string(),
            move |repo| async move { ok(git::stash_file(&repo, &path, &path).await, "") },
        ),
        "file:ignore" => act(
            p,
            ctx,
            t!("git.ignore_file").to_string(),
            move |repo| async move {
                git::ignore_file(&repo, &path).await.map(|untracked| {
                    if untracked {
                        t!("git.file_ignored_untracked").to_string()
                    } else {
                        t!("git.file_ignored").to_string()
                    }
                })
            },
        ),
        "file:open" => {
            if let Some(repo) = ctx.repo() {
                let file = repo.join(&path);
                if file.is_file() {
                    let _ = host.open_file(file, None);
                } else {
                    host.error(t!("git.file_gone"));
                }
            }
        }
        "file:diff" => show_change(p, ctx, staged, &path),
        "file:history" => {
            ctx.with(|g| {
                g.log_path = Some(path);
                g.log_rev = "HEAD".into();
                g.tab = Tab::Log;
                g.selected.clear();
            });
            refresh(p, ctx);
        }
        "file:commit_only" => commit(p, ctx, false, Some(path)),
        _ => {}
    }
}

fn commit(p: &GitPlugin, ctx: &Ctx, then_push: bool, only: Option<String>) {
    let (message, amend, options, staged) = ctx.with(|g| {
        (
            g.message.trim().to_string(),
            g.amend,
            g.options.clone(),
            g.status.as_ref().is_some_and(|s| !s.staged.is_empty()),
        )
    });
    if message.is_empty() {
        ctx.with(|g| g.say(Err(t!("git.empty_message").to_string())));
        ctx.render();
        return;
    }
    if !staged && !amend && only.is_none() {
        ctx.with(|g| g.say(Err(t!("git.err_nothing_staged").to_string())));
        ctx.render();
        return;
    }
    let branch = ctx.with(|g| {
        g.status
            .as_ref()
            .map(|s| (s.branch.clone(), s.upstream.is_some()))
    });
    let ctx2 = ctx.clone();
    act(
        p,
        ctx,
        t!("git.commit").to_string(),
        move |repo| async move {
            let done = match &only {
                Some(path) => git::commit_only(&repo, &message, amend, path, &options).await,
                None => git::commit(&repo, &message, amend, &options).await,
            };
            done?;
            ctx2.with(|g| {
                g.message.clear();
                g.amend = false;
            });
            if !then_push {
                return Ok(t!("git.committed").to_string());
            }
            let Some((branch, upstream)) = branch else {
                return Ok(t!("git.committed").to_string());
            };
            match git::push(&repo, &branch, upstream).await {
                Ok(()) => Ok(t!("git.committed_pushed").to_string()),
                Err(e) => Err(format!("{} {e}", t!("git.commit_push_failed"))),
            }
        },
    );
}

fn log_action(p: &GitPlugin, ctx: &Ctx, host: &Host, action: &str) {
    let Some(commit) = ctx.with(|g| g.commit().cloned()) else {
        return;
    };
    let shas = ctx.with(|g| g.selected.clone());
    let sha = commit.sha.clone();
    let open = |dialog: Dialog, text: String| {
        ctx.with(|g| {
            g.dialog = Some(dialog);
            g.dialog_text = text;
        });
        ctx.render();
    };
    match action {
        "log:checkout" => act(
            p,
            ctx,
            t!("git.checkout").to_string(),
            move |repo| async move { ok(history::checkout_ref(&repo, &sha).await, "") },
        ),
        "log:branch" => open(Dialog::Branch, String::new()),
        "log:tag" => open(Dialog::Tag, String::new()),
        "log:reset" => open(Dialog::Reset, String::new()),
        "log:reword" => {
            let message = ctx.with(|g| g.commit_message.clone());
            open(Dialog::Reword, message);
        }
        "log:squash" => {
            let message = ctx.with(|g| {
                g.selected
                    .iter()
                    .filter_map(|s| g.commits.iter().find(|c| &c.sha == s))
                    .map(|c| c.subject.clone())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            });
            open(Dialog::Squash, message);
        }
        "log:copy" => host.copy(sha),
        "log:diff_tree" => {
            let files = ctx.with(|g| g.commit_files.clone());
            let files = files
                .into_iter()
                .map(|c| (Source::Tree(sha.clone()), c))
                .collect();
            show_files(
                p,
                ctx,
                format!("tree:{sha}"),
                t!("git.diff_with_tree", name = short(&sha)).to_string(),
                files,
                0,
            );
        }
        "log:compare_two" => {
            if shas.len() != 2 {
                return;
            }
            let (a, b) = (shas[0].clone(), shas[1].clone());
            let ctx2 = ctx.clone();
            p.spawn(async move {
                let Some(repo) = ctx2.repo() else {
                    return;
                };
                match history::range_files(&repo, &a, &b).await {
                    Ok(files) => {
                        let files = files
                            .into_iter()
                            .map(|c| (Source::Range(a.clone(), b.clone()), c))
                            .collect();
                        let tab = DiffTab {
                            title: format!("{} ↔ {}", short(&a), short(&b)),
                            files,
                            options: Default::default(),
                            start: 0,
                        };
                        ctx2.show_diffs(format!("range:{a}..{b}"), tab).await;
                    }
                    Err(e) => ctx2.host.error(e),
                }
            });
        }
        "confirm:log:revert" | "confirm:log:cherry_pick" | "confirm:log:drop" => {
            if !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            let action = action.to_string();
            act(
                p,
                ctx,
                t!("git.tab_log").to_string(),
                move |repo| async move {
                    let result = match action.as_str() {
                        "confirm:log:revert" => history::revert(&repo, &shas).await,
                        "confirm:log:cherry_pick" => history::cherry_pick(&repo, &shas).await,
                        _ => history::drop_commits(&repo, &shas).await,
                    };
                    ok(result, "")
                },
            );
        }
        "log:fixup" | "log:squash_into" => {
            let squash = action == "log:squash_into";
            act(
                p,
                ctx,
                t!("git.fixup").to_string(),
                move |repo| async move { ok(tools::fixup(&repo, &sha, squash).await, "") },
            );
        }
        "log:rebase_from" => {
            let ctx2 = ctx.clone();
            let base = commit.parents.first().cloned();
            p.spawn(async move {
                let Some(repo) = ctx2.repo() else {
                    return;
                };
                match history::plan_from(&repo, base.as_deref()).await {
                    Ok(plan) => ctx2.with(|g| g.plan = Some((base, plan))),
                    Err(e) => ctx2.with(|g| g.say(Err(e))),
                }
                ctx2.render();
            });
        }
        "log:patch" => {
            let ctx2 = ctx.clone();
            p.spawn(async move {
                let Some(repo) = ctx2.repo() else {
                    return;
                };
                match tools::commits_patch(&repo, &shas).await {
                    Ok(patch) => {
                        ctx2.with(|g| {
                            g.patch = patch;
                            g.tab = Tab::Tools;
                            g.tool = Tool::Patch;
                        });
                    }
                    Err(e) => ctx2.with(|g| g.say(Err(e))),
                }
                ctx2.render();
            });
        }
        _ => {}
    }
}

fn dialog_ok(p: &GitPlugin, ctx: &Ctx) {
    let (dialog, text, sha, shas, mode, file) = ctx.with(|g| {
        (
            g.dialog.take(),
            g.dialog_text.trim().to_string(),
            g.commit().map(|c| c.sha.clone()),
            g.selected.clone(),
            g.reset_mode.clone(),
            g.commit_file.clone(),
        )
    });
    let (Some(dialog), Some(sha)) = (dialog, sha) else {
        return;
    };
    act(
        p,
        ctx,
        t!("git.tab_log").to_string(),
        move |repo| async move {
            match dialog {
                Dialog::Branch => ok(
                    history::create_branch(&repo, &text, Some(&sha)).await,
                    t!("git.branch_created", name = text.as_str()),
                ),
                Dialog::Tag => {
                    let (name, message) = match text.split_once('\n') {
                        Some((n, m)) => (n.trim().to_string(), Some(m.trim().to_string())),
                        None => (text.clone(), None),
                    };
                    ok(
                        history::create_tag(&repo, &name, Some(&sha), message.as_deref()).await,
                        t!("git.tag_created", name = name.as_str()),
                    )
                }
                Dialog::Reword => ok(history::reword(&repo, &sha, &text).await, ""),
                Dialog::Extract => {
                    let paths: Vec<String> = file.into_iter().collect();
                    ok(
                        tools::extract_from_commit(&repo, &sha, &paths, &text).await,
                        "",
                    )
                }
                Dialog::Squash => ok(history::squash_commits(&repo, &shas, &text).await, ""),
                Dialog::Reset => {
                    let mode = match mode.as_str() {
                        "soft" => git::ResetMode::Soft,
                        "hard" => git::ResetMode::Hard,
                        _ => git::ResetMode::Mixed,
                    };
                    ok(git::reset(&repo, &sha, mode).await, "")
                }
            }
        },
    );
}

fn ref_action(p: &GitPlugin, ctx: &Ctx, action: &str) {
    let Some(id) = ctx.with(|g| g.reference.clone()) else {
        return;
    };
    let name = ref_name(&id).to_string();
    let kind = id.split(':').next().unwrap_or_default().to_string();
    let branch = ctx.with(|g| g.refs.branches.iter().find(|b| b.name == name).cloned());
    if action.starts_with("confirm:") && !ctx.with(|g| g.confirm(action)) {
        ctx.render();
        return;
    }
    let settings = ctx.with(|g| g.settings.clone());
    match action {
        "ref:log" => {
            ctx.with(|g| {
                g.log_rev = name;
                g.tab = Tab::Log;
                g.selected.clear();
            });
            refresh(p, ctx);
        }
        "ref:compare" => {
            let ctx2 = ctx.clone();
            p.spawn(async move {
                let Some(repo) = ctx2.repo() else {
                    return;
                };
                match history::tree_diff_files(&repo, &name).await {
                    Ok(files) => {
                        let files = files
                            .into_iter()
                            .map(|c| (Source::Tree(name.clone()), c))
                            .collect();
                        let tab = DiffTab {
                            title: t!("git.diff_with_tree", name = name.as_str()).to_string(),
                            files,
                            options: Default::default(),
                            start: 0,
                        };
                        ctx2.show_diffs(format!("tree:{name}"), tab).await;
                    }
                    Err(e) => ctx2.host.error(e),
                }
            });
        }
        _ => {
            let action = action.trim_start_matches("confirm:").to_string();
            act(
                p,
                ctx,
                t!("git.tab_branches").to_string(),
                move |repo| async move {
                    let result = match action.as_str() {
                        "ref:checkout" => match &branch {
                            Some(b) => history::checkout(&repo, b).await,
                            None => history::checkout_ref(&repo, &name).await,
                        },
                        "ref:merge" => git::merge_branch(&repo, &name, settings.merge()).await,
                        "ref:rebase" => git::rebase_onto(&repo, &name).await,
                        "ref:pull" => match name.split_once('/') {
                            Some((remote, b)) if kind == "r" => {
                                git::pull(&repo, settings.pull(), Some((remote, b))).await
                            }
                            _ => git::pull(&repo, settings.pull(), None).await,
                        },
                        "ref:fetch" => git::fetch(&repo).await,
                        "ref:push" | "ref:force_push" => {
                            let upstream = branch.as_ref().and_then(|b| b.upstream.clone());
                            history::push_branch(
                                &repo,
                                &name,
                                upstream.as_deref(),
                                action == "ref:force_push",
                            )
                            .await
                        }
                        "ref:push_tag" => history::push_tag(&repo, &name).await,
                        "ref:delete_remote_tag" => history::delete_remote_tag(&repo, &name).await,
                        "ref:delete" => match kind.as_str() {
                            "t" => history::delete_tag(&repo, &name).await,
                            "r" => history::delete_remote_branch(&repo, &name).await,
                            _ => history::delete_branch(&repo, &name, false).await,
                        },
                        "ref:force_delete" => history::delete_branch(&repo, &name, true).await,
                        _ => Ok(()),
                    };
                    match result {
                        Err(e) if e == history::NOT_MERGED => {
                            Err(t!("git.branch_not_merged", name = name.as_str()).to_string())
                        }
                        other => ok(other, ""),
                    }
                },
            );
        }
    }
}

fn ref_dialog_ok(p: &GitPlugin, ctx: &Ctx) {
    let (dialog, name, message, id) = ctx.with(|g| {
        (
            g.ref_dialog.take(),
            g.ref_name.trim().to_string(),
            g.tag_message.trim().to_string(),
            g.reference.clone(),
        )
    });
    let Some(dialog) = dialog else {
        return;
    };
    if name.is_empty() || name.starts_with('-') {
        ctx.with(|g| g.say(Err(t!("git.err_branch_name").to_string())));
        ctx.render();
        return;
    }
    let from = id.as_deref().map(ref_name).map(str::to_string);
    act(
        p,
        ctx,
        t!("git.tab_branches").to_string(),
        move |repo| async move {
            match dialog {
                RefDialog::Create => ok(
                    history::create_branch(&repo, &name, from.as_deref()).await,
                    t!("git.branch_created", name = name.as_str()),
                ),
                RefDialog::Rename => {
                    let Some(old) = from else {
                        return Ok(String::new());
                    };
                    ok(tools::rename_branch(&repo, &old, &name).await, "")
                }
                RefDialog::Tag => {
                    let message = (!message.is_empty()).then_some(message.as_str());
                    ok(
                        history::create_tag(&repo, &name, from.as_deref(), message).await,
                        t!("git.tag_created", name = name.as_str()),
                    )
                }
            }
        },
    );
}

/// A tool was shown: what it needs is read.
fn tool_opened(p: &GitPlugin, ctx: &Ctx) {
    let tool = ctx.with(|g| g.tool);
    let ctx = ctx.clone();
    p.spawn(async move {
        let repo = ctx.repo();
        match tool {
            Tool::Push | Tool::Remotes => {
                let Some(repo) = repo else {
                    return;
                };
                let remotes = tools::remotes(&repo).await.unwrap_or_default();
                let remote = git::default_remote(&repo).await;
                ctx.with(|g| {
                    g.remotes = remotes;
                    if let Some(status) = &g.status {
                        if g.push.source.is_empty() || g.push.source != status.branch {
                            g.push.source = status.branch.clone();
                            g.push.target = status
                                .upstream
                                .as_deref()
                                .and_then(|u| u.split_once('/'))
                                .map_or(status.branch.clone(), |(_, b)| b.to_string());
                            g.push.set_upstream = status.upstream.is_none();
                        }
                    }
                    if g.push.remote.is_empty() {
                        g.push.remote = remote;
                    }
                });
                let (source, target) = ctx.with(|g| {
                    let target = format!("{}/{}", g.push.remote, g.push.target);
                    let known = g.refs.branches.iter().any(|b| b.remote && b.name == target);
                    (g.push.source.clone(), known.then_some(target))
                });
                if !source.is_empty() {
                    let outgoing = tools::outgoing(&repo, &source, target.as_deref())
                        .await
                        .unwrap_or_default();
                    ctx.with(|g| g.outgoing = outgoing);
                }
            }
            Tool::Merge | Tool::Rebase => {
                if let Some(repo) = repo {
                    let refs = history::refs(&repo).await.unwrap_or_default();
                    ctx.with(|g| g.refs = refs);
                }
            }
            Tool::Trees => {
                let Some(repo) = repo else {
                    return;
                };
                let submodules = tools::submodules(&repo).await.unwrap_or_default();
                let worktrees = tools::worktrees(&repo).await.unwrap_or_default();
                ctx.with(|g| {
                    g.submodules = submodules;
                    g.worktrees = worktrees;
                });
            }
            Tool::Repos => {
                let lines = repo_lines(&ctx.with(|g| g.workspace.clone())).await;
                ctx.with(|g| g.repos = lines);
            }
            Tool::Patch | Tool::Clean | Tool::Clone | Tool::File => {}
        }
        ctx.render();
    });
}

/// The repositories of the projects: each project, and the folders right
/// in it that are repositories of their own.
async fn repo_lines(workspace: &[PathBuf]) -> Vec<RepoLine> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for dir in workspace {
        if let Some(root) = git::repo_root(dir).await {
            roots.push(root);
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().join(".git").exists() {
                    roots.push(entry.path());
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    let mut lines = Vec::new();
    for root in roots {
        if let Ok(status) = git::status(&root, false).await {
            lines.push(RepoLine {
                branch: status.branch.clone(),
                ahead: status.ahead,
                behind: status.behind,
                changes: status.staged.len() + status.unstaged.len(),
                root,
            });
        }
    }
    lines
}

fn tool_event(p: &GitPlugin, ctx: &Ctx, host: &Host, action: &str, text: String, checked: bool) {
    match action {
        // Push
        "push_remote" => ctx.with(|g| g.push.remote = text),
        "push_target" => ctx.with(|g| g.push.target = text),
        "push_upstream" => {
            ctx.with(|g| g.push.set_upstream = checked);
            ctx.render();
        }
        "push_tags" => {
            ctx.with(|g| {
                if let Some((tags, _)) =
                    crate::ui::push_tags().into_iter().find(|(_, l)| *l == text)
                {
                    g.push.tags = tags;
                }
            });
            ctx.render();
        }
        "push:run" | "confirm:push:force" => {
            if action == "confirm:push:force" && !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            let mut spec = ctx.with(|g| g.push.clone());
            spec.force = action == "confirm:push:force";
            let ctx2 = ctx.clone();
            act(p, ctx, t!("git.push").to_string(), move |repo| async move {
                match tools::push(&repo, &spec).await {
                    Ok(()) => Ok(t!("git.pushed").to_string()),
                    Err(e) if tools::is_rejected(&e) => {
                        ctx2.with(|g| g.armed = Some("rejected".into()));
                        Err(t!("git.rejected_text").to_string())
                    }
                    Err(e) => Err(e),
                }
            });
        }
        "push:merge" | "push:rebase" => {
            let spec = ctx.with(|g| g.push.clone());
            let mode = if action == "push:rebase" {
                git::PullMode::Rebase
            } else {
                git::PullMode::Merge
            };
            act(p, ctx, t!("git.push").to_string(), move |repo| async move {
                ok(
                    tools::update_and_push(&repo, &spec, mode).await,
                    t!("git.pushed"),
                )
            });
        }

        // The file in front
        "file_rev" => ctx.with(|g| g.file_rev = text),
        "rev:compare" | "confirm:rev:get" => {
            if action == "confirm:rev:get" && !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            let (file, rev) = ctx.with(|g| (g.current_file.clone(), g.file_rev.trim().to_string()));
            let Some(file) = file else {
                return;
            };
            let get = action == "confirm:rev:get";
            let ctx2 = ctx.clone();
            p.spawn(async move {
                let Some(root) = git::repo_root(&file).await else {
                    ctx2.host.error(t!("git.file_no_repo"));
                    return;
                };
                let Some(inner) = relative(&root, &file) else {
                    return;
                };
                let rev = match git::verify_rev(&root, &rev).await {
                    Ok(_) => rev,
                    Err(e) => {
                        ctx2.with(|g| g.say(Err(e)));
                        ctx2.render();
                        return;
                    }
                };
                if get {
                    let result = ok(
                        tools::get_from_revision(&root, &rev, &inner).await,
                        t!("git.file_got"),
                    );
                    ctx2.with(|g| g.say(result));
                    ctx2.refresh().await;
                    return;
                }
                let change = Change {
                    path: inner.clone(),
                    from: None,
                    kind: git::Kind::Modified,
                };
                let tab = DiffTab {
                    title: t!("git.diff_with_tree", name = rev.as_str()).to_string(),
                    files: vec![(Source::Tree(rev.clone()), change)],
                    options: Default::default(),
                    start: 0,
                };
                ctx2.with(|g| g.repo = Some(root));
                ctx2.show_diffs(format!("file:{inner}@{rev}"), tab).await;
            });
        }
        "rev:history" => {
            let file = ctx.with(|g| g.current_file.clone());
            command(
                p,
                ctx,
                host,
                "git.file_history",
                &Context {
                    file,
                    workspace: Vec::new(),
                },
            );
        }
        "rev:lines" => command(
            p,
            ctx,
            host,
            "git.file_history_selection",
            &Context::default(),
        ),
        "rev:blame" => p.blame(),

        // Remotes
        "remotes" => {
            ctx.with(|g| {
                if let Some(r) = g.remotes.iter().find(|r| r.name == text).cloned() {
                    g.remote_edit = Some(r.name.clone());
                    g.remote_name = r.name;
                    g.remote_url = r.url;
                }
            });
            ctx.render();
        }
        "remote:new" => {
            ctx.with(|g| {
                g.remote_edit = None;
                g.remote_name.clear();
                g.remote_url.clear();
            });
            ctx.render();
        }
        "remote_name" => ctx.with(|g| g.remote_name = text),
        "remote_url" => ctx.with(|g| g.remote_url = text),
        "remote:save" | "confirm:remote:remove" => {
            if action == "confirm:remote:remove" && !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            let (old, name, url) = ctx.with(|g| {
                (
                    g.remote_edit.clone(),
                    g.remote_name.trim().to_string(),
                    g.remote_url.trim().to_string(),
                )
            });
            let remove = action == "confirm:remote:remove";
            let ctx2 = ctx.clone();
            act(
                p,
                ctx,
                t!("git.remotes").to_string(),
                move |repo| async move {
                    let result = match (&old, remove) {
                        (Some(old), true) => tools::remove_remote(&repo, old).await,
                        (Some(old), false) => tools::edit_remote(&repo, old, &name, &url).await,
                        (None, _) => tools::add_remote(&repo, &name, &url).await,
                    };
                    let remotes = tools::remotes(&repo).await.unwrap_or_default();
                    ctx2.with(|g| {
                        g.remotes = remotes;
                        g.remote_edit = None;
                        g.remote_name.clear();
                        g.remote_url.clear();
                    });
                    ok(result, "")
                },
            );
        }

        // Patches
        "patch:local" | "patch:commits" => {
            let shas = ctx.with(|g| g.selected.clone());
            let local = action == "patch:local";
            let ctx2 = ctx.clone();
            p.spawn(async move {
                let Some(repo) = ctx2.repo() else {
                    return;
                };
                let patch = if local {
                    tools::local_patch(&repo).await
                } else {
                    tools::commits_patch(&repo, &shas).await
                };
                ctx2.with(|g| match patch {
                    Ok(patch) if patch.trim().is_empty() => {
                        g.say(Err(t!("git.err_empty_patch").to_string()))
                    }
                    Ok(patch) => g.patch = patch,
                    Err(e) => g.say(Err(e)),
                });
                ctx2.render();
            });
        }
        "patch_text" => ctx.with(|g| g.patch = text),
        "patch:copy" => {
            host.copy(ctx.with(|g| g.patch.clone()));
            ctx.with(|g| g.say(Ok(t!("git.patch_copied").to_string())));
            ctx.render();
        }
        "patch:save" => {
            let patch = ctx.with(|g| g.patch.clone());
            let ctx2 = ctx.clone();
            let host = host.clone();
            p.spawn(async move {
                if let Ok(true) = host.save_file("changes.patch", patch) {
                    ctx2.with(|g| g.say(Ok(t!("git.patch_saved").to_string())));
                    ctx2.render();
                }
            });
        }
        "patch:apply_file" => {
            let ctx2 = ctx.clone();
            let host = host.clone();
            p.spawn(async move {
                let Ok(Some(file)) = host.pick_file(t!("git.patch_apply"), &["patch", "diff"])
                else {
                    return;
                };
                match std::fs::read_to_string(&file) {
                    Ok(text) => {
                        ctx2.with(|g| g.patch = text.clone());
                        apply_patch_now(&ctx2, text).await;
                    }
                    Err(e) => {
                        ctx2.with(|g| g.say(Err(e.to_string())));
                        ctx2.render();
                    }
                }
            });
        }
        "patch:apply" => {
            let text = ctx.with(|g| g.patch.clone());
            let ctx2 = ctx.clone();
            p.spawn(async move { apply_patch_now(&ctx2, text).await });
        }

        // Merge and rebase with options
        "merge_branch" => {
            ctx.with(|g| g.merge_branch = text);
            ctx.render();
        }
        "merge_mode" => {
            ctx.with(|g| {
                if let Some(mode) = crate::ui::MERGE_MODES.iter().find(|(_, l)| l() == text) {
                    g.merge.mode = match mode.0 {
                        "no-ff" => git::MergeMode::NoFastForward,
                        "ff-only" => git::MergeMode::FastForwardOnly,
                        "squash" => git::MergeMode::Squash,
                        _ => git::MergeMode::Default,
                    };
                }
            });
            ctx.render();
        }
        "merge_no_commit" | "merge_no_verify" | "merge_unrelated" => {
            ctx.with(|g| match action {
                "merge_no_commit" => g.merge.no_commit = checked,
                "merge_no_verify" => g.merge.no_verify = checked,
                _ => g.merge.allow_unrelated = checked,
            });
            ctx.render();
        }
        "merge_message" => ctx.with(|g| g.merge.message = text),
        "merge:run" => {
            let (branch, options) = ctx.with(|g| (g.merge_branch.clone(), g.merge.clone()));
            if branch.is_empty() {
                return;
            }
            act(
                p,
                ctx,
                t!("git.merge").to_string(),
                move |repo| async move { ok(tools::merge(&repo, &branch, &options).await, "") },
            );
        }
        "rebase_branch" => {
            ctx.with(|g| g.rebase_branch = text);
            ctx.render();
        }
        "rebase_onto" => ctx.with(|g| g.rebase.onto = text),
        "rebase_merges" | "rebase_keep_empty" | "rebase_root" | "rebase_update_refs"
        | "rebase_autosquash" => {
            ctx.with(|g| {
                let o = &mut g.rebase;
                match action {
                    "rebase_merges" => o.rebase_merges = checked,
                    "rebase_keep_empty" => o.keep_empty = checked,
                    "rebase_root" => o.root = checked,
                    "rebase_update_refs" => o.update_refs = checked,
                    _ => o.autosquash = checked,
                }
            });
            ctx.render();
        }
        "rebase:run" => {
            let (branch, options) = ctx.with(|g| (g.rebase_branch.clone(), g.rebase.clone()));
            if branch.is_empty() && !options.root {
                return;
            }
            act(
                p,
                ctx,
                t!("git.rebase").to_string(),
                move |repo| async move { ok(tools::rebase(&repo, &branch, &options).await, "") },
            );
        }

        // Clean
        "clean_ignored" => {
            ctx.with(|g| {
                g.clean_ignored = checked;
                g.clean.clear();
            });
            ctx.render();
        }
        "clean:preview" => {
            let ignored = ctx.with(|g| g.clean_ignored);
            let ctx2 = ctx.clone();
            p.spawn(async move {
                let Some(repo) = ctx2.repo() else {
                    return;
                };
                match tools::clean_preview(&repo, ignored).await {
                    Ok(files) if files.is_empty() => {
                        ctx2.with(|g| g.say(Ok(t!("git.clean_nothing").to_string())))
                    }
                    Ok(files) => ctx2.with(|g| g.clean = files),
                    Err(e) => ctx2.with(|g| g.say(Err(e))),
                }
                ctx2.render();
            });
        }
        "confirm:clean:run" => {
            if !ctx.with(|g| g.confirm(action)) {
                ctx.render();
                return;
            }
            let ignored = ctx.with(|g| {
                g.clean.clear();
                g.clean_ignored
            });
            act(
                p,
                ctx,
                t!("git.clean_title").to_string(),
                move |repo| async move { ok(tools::clean_run(&repo, ignored).await, "") },
            );
        }

        // Submodules and worktrees
        "trees:update" => {
            let ctx2 = ctx.clone();
            act(
                p,
                ctx,
                t!("git.submodules_update").to_string(),
                move |repo| async move {
                    let result = tools::submodules_update(&repo).await;
                    let submodules = tools::submodules(&repo).await.unwrap_or_default();
                    ctx2.with(|g| g.submodules = submodules);
                    ok(result, "")
                },
            );
        }
        "worktrees" | "repos" => {
            let root = PathBuf::from(&text);
            ctx.with(|g| {
                g.repo = Some(root);
                g.tab = Tab::Commit;
                g.selected.clear();
                g.picked = None;
            });
            refresh(p, ctx);
        }

        // Clone
        "clone_url" => ctx.with(|g| g.clone_url = text),
        "clone_dest" => ctx.with(|g| g.clone_dest = text),
        "clone:run" => {
            let (url, dest) = ctx.with(|g| {
                (
                    g.clone_url.trim().to_string(),
                    g.clone_dest.trim().to_string(),
                )
            });
            if url.is_empty() {
                ctx.with(|g| g.say(Err(t!("git.err_clone_url").to_string())));
                ctx.render();
                return;
            }
            let dest = PathBuf::from(dest);
            if dest.as_os_str().is_empty()
                || dest.exists()
                || !dest
                    .parent()
                    .is_some_and(|p| p.as_os_str().is_empty() || p.is_dir())
            {
                ctx.with(|g| g.say(Err(t!("git.err_clone_dest").to_string())));
                ctx.render();
                return;
            }
            let ctx2 = ctx.clone();
            p.spawn(async move {
                ctx2.with(|g| g.busy = Some(t!("git.cloning").to_string()));
                ctx2.render();
                let result = tools::clone_repo(&url, &dest).await;
                ctx2.with(|g| {
                    g.busy = None;
                    match result {
                        Ok(()) => {
                            g.repo = Some(dest.clone());
                            g.tab = Tab::Commit;
                            g.clone_url.clear();
                            g.clone_dest.clear();
                            g.say(Ok(
                                t!("git.cloned_in", dir = dest.display().to_string()).to_string()
                            ));
                        }
                        Err(e) => g.say(Err(e)),
                    }
                });
                ctx2.refresh().await;
            });
        }

        // Repositories of the projects
        "repos:fetch" | "repos:pull" | "repos:push" => {
            let (lines, mode) = ctx.with(|g| (g.repos.clone(), g.settings.pull()));
            let action = action.to_string();
            let ctx2 = ctx.clone();
            act(p, ctx, t!("git.repos").to_string(), move |_| async move {
                let mut said = Vec::new();
                for line in &lines {
                    let name = line
                        .root
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let result = match action.as_str() {
                        "repos:fetch" => git::fetch(&line.root).await,
                        "repos:pull" => git::pull(&line.root, mode, None).await,
                        _ => {
                            let upstream = git::status(&line.root, false)
                                .await
                                .is_ok_and(|s| s.upstream.is_some());
                            git::push(&line.root, &line.branch, upstream).await
                        }
                    };
                    said.push(match result {
                        Ok(()) => t!("git.repo_done", name = name.as_str()).to_string(),
                        Err(e) => format!("{name}: {e}"),
                    });
                }
                let workspace = ctx2.with(|g| g.workspace.clone());
                let lines = repo_lines(&workspace).await;
                ctx2.with(|g| g.repos = lines);
                Ok(said.join(" · "))
            });
        }
        _ => {}
    }
}

/// The file as it was before the commit of the line of the cursor, in a
/// tab of its own, with its blame.
async fn blame_previous(ctx: &Ctx, host: &Host) {
    let Ok(file) = host.active_file() else {
        return;
    };
    let line = file.lines.map_or(0, |(first, _)| first.saturating_sub(1));
    let Some(blame) = ctx.with(|g| g.blame.clone()) else {
        return;
    };
    let Some(sha) = blame
        .shas
        .get(line)
        .filter(|s| !s.chars().all(|c| c == '0'))
        .cloned()
    else {
        return;
    };
    let rev = format!("{sha}^");
    let options = ctx.with(|g| g.blame_options.clone());
    let found = async {
        let lines = history::blame_with(&blame.repo, Some(&rev), &blame.relative, &options).await?;
        let text = history::file_at(&blame.repo, &rev, &blame.relative).await?;
        Ok::<_, String>((lines, text))
    };
    match found.await {
        Ok((lines, text)) => {
            let name = blame.relative.rsplit('/').next().unwrap_or(&blame.relative);
            let extension = name.rsplit_once('.').map_or("", |(_, e)| e);
            let title = format!("{name} @ {}", short(&sha));
            match host.open_text(title, text, extension) {
                Ok(path) => {
                    host.annotate(&path, Some(crate::annotations(&lines)));
                    ctx.with(|g| {
                        g.blame = Some(crate::state::Blame {
                            path,
                            repo: blame.repo.clone(),
                            relative: blame.relative.clone(),
                            shas: lines.iter().map(|l| l.sha.clone()).collect(),
                        })
                    });
                }
                Err(e) => host.error(e.message),
            }
        }
        Err(e) => host.error(e),
    }
}

async fn apply_patch_now(ctx: &Ctx, text: String) {
    let Some(repo) = ctx.repo() else {
        return;
    };
    ctx.act(t!("git.patch_apply").to_string(), async move {
        if text.trim().is_empty() {
            return Err(t!("git.err_empty_patch").to_string());
        }
        ok(
            tools::apply_patch(&repo, &text).await,
            t!("git.patch_applied"),
        )
    })
    .await;
}

/// A button of a diff tab of the plugin.
pub async fn diff_action(ctx: &Ctx, params: DiffActionParams) {
    let Some(repo) = ctx.repo() else {
        return;
    };
    let Some(tab) = ctx.with(|g| g.diffs.get(&params.tab).cloned()) else {
        return;
    };
    let Some((source, change)) = tab.files.get(params.file).cloned() else {
        return;
    };
    ctx.with(|g| {
        if let Some(t) = g.diffs.get_mut(&params.tab) {
            t.start = params.file;
        }
    });
    match params.action {
        DiffAction::ApplyToIndex { patch, reverse } => {
            ctx.act(t!("git.stage").to_string(), async {
                ok(git::apply_patch(&repo, &patch, reverse).await, "")
            })
            .await;
        }
        DiffAction::Rollback { block } => {
            let options = tab.options;
            ctx.act(t!("git.rollback").to_string(), async {
                let diff = git::diff_with(
                    &repo,
                    &source,
                    &change.path,
                    change.from.as_deref(),
                    options,
                )
                .await?;
                ok(
                    git::rollback_block(&repo, &change.path, &diff, block).await,
                    "",
                )
            })
            .await;
        }
        DiffAction::Options {
            ignore_blanks,
            context,
        } => {
            ctx.with(|g| {
                if let Some(t) = g.diffs.get_mut(&params.tab) {
                    t.options = git::DiffOptions {
                        context,
                        ignore_blanks,
                    };
                }
            });
            ctx.send_diffs(&params.tab).await;
        }
        DiffAction::ToggleCompare => {
            ctx.with(|g| {
                if let Some(t) = g.diffs.get_mut(&params.tab) {
                    if let Some((s, _)) = t.files.get_mut(params.file) {
                        *s = match s.clone() {
                            Source::Commit(rev) => Source::Tree(rev),
                            Source::Tree(rev) => Source::Commit(rev),
                            other => other,
                        };
                    }
                }
            });
            ctx.send_diffs(&params.tab).await;
        }
    }
}
