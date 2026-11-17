//! The Git plugin of SimpleEdit: the changes to commit, the stashes, the log
//! with its graph, the branches and tags, the tools (push, remotes, patches,
//! merge and rebase with options, clean, submodules and worktrees, clone,
//! the repositories of the projects). In the editor: the colors of the
//! files, the marks of the changed lines, the blame, the diffs and the
//! conflicts in tabs.

#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod actions;
mod fsutil;
mod git;
mod state;
mod ui;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use simpleedit_plugin::protocol::{
    AnnotationLine, Annotations, Context, DiffActionParams, DiffFile, DiffMode, DiffTabParams,
    FileDecoration, FileState as Decoration, InitializeParams, MergeParams, Settings as Editor,
    StatusParams, UiEvent,
};
use simpleedit_plugin::{run, Host, Plugin};

use git::{history, Change, Source};
use state::{Blame, DiffTab, Git, Settings, Tab};

pub const PANEL: &str = "git";
pub const SETTINGS: &str = "settings";

/// How often the panel looks at the working tree while it is open.
const TICK: Duration = Duration::from_secs(4);

/// The state, and how to talk to the editor, for the work done in the
/// background.
#[derive(Clone)]
pub struct Ctx {
    pub shared: Arc<Mutex<Git>>,
    pub host: Host,
    pub data_dir: Arc<Mutex<PathBuf>>,
    pub open: Arc<std::sync::atomic::AtomicBool>,
}

pub fn lock(shared: &Arc<Mutex<Git>>) -> MutexGuard<'_, Git> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

impl Ctx {
    pub fn with<R>(&self, f: impl FnOnce(&mut Git) -> R) -> R {
        f(&mut lock(&self.shared))
    }

    pub fn repo(&self) -> Option<PathBuf> {
        self.with(|g| g.repo.clone())
    }

    pub fn render(&self) {
        let g = lock(&self.shared);
        self.host.render(PANEL, ui::panel(&g));
        if let Some(status) = &g.status {
            self.host.status(StatusParams {
                id: "branch".into(),
                text: Some(format!(
                    "\u{e0a0} {}",
                    state::branch_label(&status.branch, status.ahead, status.behind)
                )),
                tooltip: Some(t!("git.title").to_string()),
                command: Some("git.branches".into()),
            });
        } else {
            self.host.clear_status("branch");
        }
    }

    pub fn render_settings(&self) {
        let g = lock(&self.shared);
        self.host.render(SETTINGS, ui::settings(&g.settings));
    }

    pub fn save_settings(&self) {
        let settings = self.with(|g| g.settings.clone());
        let path = self.data_dir.lock().map(|d| d.join("settings.json"));
        if let (Ok(path), Ok(json)) = (path, serde_json::to_string_pretty(&settings)) {
            let _ = fsutil::write_atomic(&path, json.as_bytes(), false);
        }
    }

    /// Runs a git operation: the panel waits, says how it went, and looks
    /// again at everything.
    pub async fn act<F>(&self, label: String, op: F)
    where
        F: Future<Output = Result<String, String>>,
    {
        self.with(|g| {
            g.busy = Some(label);
            g.armed = None;
        });
        self.render();
        let result = op.await;
        self.with(|g| {
            g.busy = None;
            g.say(result);
        });
        self.refresh().await;
    }

    /// Looks again at the repository: its state, refs, stashes, log; then
    /// what the editor shows of it.
    pub async fn refresh(&self) {
        let Some(repo) = self.repo() else {
            self.render();
            self.decorate().await;
            return;
        };
        let (rev, limit, path, search) = self.with(|g| {
            let search =
                (!g.search.trim().is_empty()).then(|| (g.search_mode, g.search.trim().to_string()));
            (g.log_rev.clone(), g.limit, g.log_path.clone(), search)
        });
        let status = git::status(&repo, true).await;
        let refs = history::refs(&repo).await.unwrap_or_default();
        let stashes = history::stashes(&repo).await.unwrap_or_default();
        let search_ref = search.as_ref().map(|(m, s)| (*m, s.as_str()));
        let filter = self.with(|g| g.filter.clone());
        let commits =
            history::log_filtered(&repo, &rev, limit, path.as_deref(), search_ref, &filter)
                .await
                .unwrap_or_default();
        let in_head = history::head_commits(&repo, limit.max(1))
            .await
            .unwrap_or_default();
        let recent = git::tools::recent_messages(&repo, 20)
            .await
            .unwrap_or_default();
        let reveal = self.with(|g| {
            match status {
                Ok(status) => g.status = Some(status),
                Err(error) => g.notice = Some((error, true)),
            }
            g.refs = refs;
            g.stashes = stashes;
            g.recent = recent;
            g.in_head = in_head;
            if g.stash.is_some_and(|i| i >= g.stashes.len()) {
                g.stash = None;
                g.stash_files.clear();
            }
            g.rows = git::graph::layout(&commits);
            g.commits = commits;
            g.selected
                .retain(|sha| g.commits.iter().any(|c| &c.sha == sha));
            g.reveal.take()
        });
        if let Some(sha) = reveal {
            if self.with(|g| g.commits.iter().any(|c| c.sha == sha)) {
                self.pick_commit(sha).await;
            } else if self.with(|g| g.log_rev != sha) {
                // Not in this history: the history of that commit.
                self.with(|g| {
                    g.log_rev = sha.clone();
                    g.reveal = Some(sha);
                });
                Box::pin(self.refresh()).await;
                return;
            }
        }
        self.render();
        self.decorate().await;
        self.send_base().await;
        self.resend_working_diffs().await;
    }

    /// The state of the working tree only (often, while the panel is open).
    pub async fn tick(&self) {
        let Some(repo) = self.repo() else {
            return;
        };
        let Ok(status) = git::status(&repo, true).await else {
            return;
        };
        let changed = self.with(|g| {
            let changed = g.status.as_ref() != Some(&status);
            g.status = Some(status);
            changed
        });
        if changed {
            self.render();
            self.decorate().await;
            self.resend_working_diffs().await;
        }
    }

    /// The colors of the files of the projects (and of the repository shown).
    pub async fn decorate(&self) {
        let (mut dirs, repo) = self.with(|g| (g.workspace.clone(), g.repo.clone()));
        dirs.extend(repo);
        let states = git::file_states_of(dirs).await;
        let files = states
            .into_iter()
            .map(|(path, state, folder)| FileDecoration {
                path,
                state: match state {
                    git::FileState::Modified => Decoration::Modified,
                    git::FileState::Added => Decoration::Added,
                    git::FileState::Untracked => Decoration::Untracked,
                    git::FileState::Conflicted => Decoration::Conflicted,
                    git::FileState::Ignored => Decoration::Ignored,
                },
                folder,
            })
            .collect();
        self.host.decorate(files);
    }

    /// What the file in front is compared with: what HEAD has of it.
    pub async fn send_base(&self) {
        let Some(path) = self.with(|g| g.current_file.clone()) else {
            return;
        };
        base_of(&self.host, &path).await;
    }

    /// Picks a commit of the log: its files and message are read.
    pub async fn pick_commit(&self, sha: String) {
        let Some(repo) = self.repo() else {
            return;
        };
        let commit = self.with(|g| {
            if !g.selected.contains(&sha) {
                g.selected = vec![sha.clone()];
            }
            g.tab = Tab::Log;
            g.commits.iter().find(|c| c.sha == sha).cloned()
        });
        let Some(commit) = commit else {
            return;
        };
        let files = history::commit_files(&repo, &commit)
            .await
            .unwrap_or_default();
        let message = history::message(&repo, &sha).await.unwrap_or_default();
        let signature = history::signature(&repo, &sha).await;
        let containing = history::branches_containing(&repo, &sha)
            .await
            .unwrap_or_default();
        let pushed = history::on_remote(&repo, &sha).await;
        self.with(|g| {
            if g.selected.last() == Some(&sha) {
                g.commit_files = files;
                g.commit_message = message;
                g.signature = signature;
                g.containing = containing;
                g.pushed = pushed;
                g.commit_file = None;
            }
        });
        self.render();
    }

    /// Shows diffs in a tab of the editor (the tab of that id, if open).
    pub async fn show_diffs(&self, id: String, tab: DiffTab) {
        self.with(|g| {
            g.diffs.insert(id.clone(), tab);
        });
        self.send_diffs(&id).await;
    }

    async fn send_diffs(&self, id: &str) {
        let (Some(repo), Some(tab)) = (self.repo(), self.with(|g| g.diffs.get(id).cloned())) else {
            return;
        };
        let mut files = Vec::new();
        for (source, change) in &tab.files {
            let patch = git::patch_with(
                &repo,
                source,
                &change.path,
                change.from.as_deref(),
                tab.options,
            )
            .await
            .unwrap_or_default();
            let mode = match source {
                Source::Unstaged => DiffMode::Unstaged,
                Source::Staged => DiffMode::Staged,
                Source::Commit(_) => DiffMode::Commit,
                Source::Tree(_) => DiffMode::Against,
                Source::Untracked | Source::Stash(_) | Source::Range(..) => DiffMode::Plain,
            };
            files.push(DiffFile {
                path: change.path.clone(),
                from: change.from.clone(),
                patch,
                mode,
                file: Some(repo.join(&change.path)),
            });
        }
        let _ = self.host.open_diff_tab(DiffTabParams {
            tab: id.to_string(),
            title: tab.title,
            files,
            start: tab.start,
        });
    }

    /// The diff tabs of the working tree follow it.
    pub async fn resend_working_diffs(&self) {
        let ids: Vec<String> = self.with(|g| {
            let status = g.status.clone().unwrap_or_default();
            for (id, tab) in g.diffs.iter_mut() {
                // The lists of the Commit tab follow what is staged or not.
                let files = match id.as_str() {
                    "changes:staged" => Some(&status.staged),
                    "changes:unstaged" => Some(&status.unstaged),
                    _ => None,
                };
                if let Some(files) = files {
                    tab.files = files
                        .iter()
                        .map(|c| (source_of(false, c, id == "changes:staged"), c.clone()))
                        .collect();
                }
            }
            g.diffs
                .iter()
                .filter(|(_, t)| {
                    t.files.iter().any(|(s, _)| {
                        matches!(
                            s,
                            Source::Unstaged | Source::Staged | Source::Untracked | Source::Tree(_)
                        )
                    })
                })
                .map(|(id, _)| id.clone())
                .collect()
        });
        for id in ids {
            self.send_diffs(&id).await;
        }
    }
}

/// How a file of the Commit tab is diffed.
pub fn source_of(_: bool, change: &Change, staged: bool) -> Source {
    if staged {
        Source::Staged
    } else if change.kind == git::Kind::Untracked {
        Source::Untracked
    } else {
        Source::Unstaged
    }
}

/// Tells the editor what a file is compared with.
async fn base_of(host: &Host, path: &Path) {
    let Some(root) = git::repo_root(path).await else {
        host.clear_base(path);
        return;
    };
    let Some(relative) = relative(&root, path) else {
        host.clear_base(path);
        return;
    };
    host.set_base(path, git::head_text(&root, &relative).await);
}

/// A path inside a repository, as git writes it.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let inner = path.strip_prefix(&root).ok()?;
    Some(inner.to_string_lossy().replace('\\', "/"))
}

/// The blame of a file, as a column beside its lines.
pub fn annotations(lines: &[history::BlameLine]) -> Annotations {
    const TINTS: [[u8; 3]; 6] = [
        [0x6e, 0xa8, 0xff],
        [0x8f, 0xd1, 0x8f],
        [0xff, 0x9b, 0x6b],
        [0xc6, 0x8a, 0xe8],
        [0xe6, 0xc0, 0x6a],
        [0x5e, 0xcf, 0xd1],
    ];
    let mut details: Vec<Vec<String>> = Vec::new();
    let mut index_of: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for l in lines.iter().filter(|l| l.committed()) {
        index_of.entry(l.sha.as_str()).or_insert_with(|| {
            details.push(commit_card(l));
            details.len() - 1
        });
    }
    let lines = lines
        .iter()
        .map(|l| {
            if !l.committed() {
                return AnnotationLine {
                    text: t!("git.not_committed").to_string(),
                    tint: None,
                    detail: None,
                };
            }
            let n = l
                .sha
                .bytes()
                .take(4)
                .fold(0usize, |a, b| a * 31 + b as usize);
            AnnotationLine {
                text: format!("{}  {}", state::format_date(l.time), l.author),
                tint: Some(TINTS[n % TINTS.len()]),
                detail: index_of.get(l.sha.as_str()).copied(),
            }
        })
        .collect();
    Annotations { lines, details }
}

/// What a pointer resting on a line of the blame shows.
fn commit_card(line: &history::BlameLine) -> Vec<String> {
    let (name, email) = (line.author.as_str(), line.email.as_str());
    let date = state::format_time(line.time);
    let mut card = vec![
        t!("git.blame_commit", sha = line.sha.as_str()).to_string(),
        t!("git.blame_author", name = name, email = email).to_string(),
        t!("git.blame_date", date = date.as_str()).to_string(),
        String::new(),
    ];
    let message: &str = if line.message.is_empty() {
        &line.summary
    } else {
        &line.message
    };
    for paragraph in message.lines() {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > 72 {
                card.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        card.push(current);
    }
    card
}

struct GitPlugin {
    ctx: Option<Ctx>,
    runtime: tokio::runtime::Runtime,
}

impl GitPlugin {
    fn ctx(&self) -> Ctx {
        self.ctx.clone().expect("initialized")
    }

    /// Work in the background.
    fn spawn<F>(&self, f: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(f);
    }

    /// Finds the repository of a file (or of the first project).
    fn follow(&self, path: Option<PathBuf>) {
        let ctx = self.ctx();
        self.spawn(async move {
            let workspace = ctx.with(|g| g.workspace.clone());
            let start = path.clone().or_else(|| workspace.first().cloned());
            let root = match &start {
                Some(p) => git::repo_root(p).await,
                None => None,
            };
            let changed = ctx.with(|g| {
                g.searched = true;
                g.init_dir = start.as_ref().and_then(|p| init_target(p, &g.workspace));
                if root.is_some() && g.repo != root {
                    g.repo = root.clone();
                    g.status = None;
                    g.picked = None;
                    g.selected.clear();
                    g.commit_files.clear();
                    g.reference = None;
                    g.log_path = None;
                    g.log_rev = "HEAD".into();
                    true
                } else {
                    false
                }
            });
            if changed {
                ctx.refresh().await;
            } else {
                ctx.render();
                ctx.send_base().await;
            }
        });
    }
}

/// Where "Initialize a repository" makes one for a file or folder: the
/// project it is in, else its folder. Never the home folder or above.
pub fn init_target(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    let target = roots
        .iter()
        .filter(|r| dir.starts_with(r))
        .max_by_key(|r| r.components().count())
        .cloned()
        .unwrap_or(dir);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if home.is_some_and(|h| h.starts_with(&target)) || target.parent().is_none() {
        return None;
    }
    Some(target)
}

impl Plugin for GitPlugin {
    fn initialize(&mut self, host: &Host, params: &InitializeParams) {
        rust_i18n::set_locale(&params.settings.locale);
        fsutil::set_data_dir(params.data_dir.clone());
        let mut git = Git::new();
        git.workspace = params.workspace.clone();
        if let Ok(text) = std::fs::read_to_string(params.data_dir.join("settings.json")) {
            git.settings = serde_json::from_str::<Settings>(&text).unwrap_or_default();
        }
        let ctx = Ctx {
            shared: Arc::new(Mutex::new(git)),
            host: host.clone(),
            data_dir: Arc::new(Mutex::new(params.data_dir.clone())),
            open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        self.ctx = Some(ctx.clone());
        self.follow(None);
        // The working tree while the panel is open, and the fetches.
        self.spawn(async move {
            let mut fetched = Instant::now();
            loop {
                tokio::time::sleep(TICK).await;
                if ctx.open.load(std::sync::atomic::Ordering::Relaxed) {
                    ctx.tick().await;
                }
                let minutes = ctx.with(|g| g.settings.auto_fetch_minutes);
                if minutes > 0 && fetched.elapsed() >= Duration::from_secs(minutes as u64 * 60) {
                    fetched = Instant::now();
                    if let Some(repo) = ctx.repo() {
                        if git::fetch(&repo).await.is_ok() {
                            ctx.refresh().await;
                        }
                    }
                }
            }
        });
    }

    fn command(&mut self, host: &Host, command: &str, context: &Context) {
        let ctx = self.ctx();
        ctx.with(|g| {
            if !context.workspace.is_empty() {
                g.workspace = context.workspace.clone();
            }
            if context.file.is_some() {
                g.current_file = context.file.clone();
            }
        });
        actions::command(self, &ctx, host, command, context);
    }

    fn panel_opened(&mut self, _: &Host, panel: &str) {
        let ctx = self.ctx();
        match panel {
            PANEL => {
                ctx.open.store(true, std::sync::atomic::Ordering::Relaxed);
                let file = ctx.with(|g| g.current_file.clone());
                self.follow(file);
                let refresh = ctx.clone();
                self.spawn(async move { refresh.refresh().await });
            }
            SETTINGS => ctx.render_settings(),
            _ => {}
        }
    }

    fn panel_closed(&mut self, _: &Host, panel: &str) {
        if panel == PANEL {
            self.ctx()
                .open
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn ui_event(&mut self, host: &Host, event: &UiEvent) {
        let ctx = self.ctx();
        if event.panel == SETTINGS {
            actions::settings_event(&ctx, event);
            return;
        }
        actions::event(self, &ctx, host, event);
    }

    fn active_file_changed(&mut self, host: &Host, path: Option<&Path>) {
        let ctx = self.ctx();
        ctx.with(|g| g.current_file = path.map(Path::to_path_buf));
        // The blame of another file goes.
        let gone = ctx.with(|g| {
            let other = g
                .blame
                .as_ref()
                .is_some_and(|b| Some(b.path.as_path()) != path);
            if other {
                g.blame.take()
            } else {
                None
            }
        });
        if let Some(blame) = gone {
            host.annotate(&blame.path, None);
        }
        if let Some(path) = path {
            self.follow(Some(path.to_path_buf()));
        }
    }

    fn file_saved(&mut self, _: &Host, _: &Path) {
        let ctx = self.ctx();
        self.spawn(async move { ctx.tick().await });
    }

    fn file_deleted(&mut self, _: &Host, _: &Path, _: Option<&str>) {
        let ctx = self.ctx();
        self.spawn(async move { ctx.refresh().await });
    }

    fn file_renamed(&mut self, _: &Host, _: &Path, _: &Path) {
        let ctx = self.ctx();
        self.spawn(async move { ctx.refresh().await });
    }

    fn settings_changed(&mut self, _: &Host, settings: &Editor) {
        rust_i18n::set_locale(&settings.locale);
        let ctx = self.ctx();
        ctx.render();
        ctx.render_settings();
    }

    fn tab_closed(&mut self, _: &Host, tab: &str) {
        self.ctx().with(|g| {
            g.diffs.remove(tab);
        });
    }

    fn diff_action(&mut self, _: &Host, params: &DiffActionParams) {
        let ctx = self.ctx();
        let params = params.clone();
        self.spawn(async move { actions::diff_action(&ctx, params).await });
    }

    fn merge_resolved(&mut self, _: &Host, params: &MergeParams) {
        let ctx = self.ctx();
        let path = params.path.clone();
        self.spawn(async move {
            let Some(repo) = ctx.repo() else {
                return;
            };
            let Some(relative) = relative(&repo, &path) else {
                return;
            };
            ctx.act(t!("git.resolve").to_string(), async {
                git::stage(&repo, &[relative]).await.map(|()| String::new())
            })
            .await;
        });
    }

    fn annotation_clicked(&mut self, host: &Host, path: &Path, line: usize) {
        let ctx = self.ctx();
        let sha = ctx.with(|g| {
            let blame = g.blame.as_ref().filter(|b| b.path == path)?;
            blame.shas.get(line).cloned()
        });
        let Some(sha) = sha.filter(|s| !s.chars().all(|c| c == '0')) else {
            return;
        };
        ctx.with(|g| {
            g.tab = Tab::Log;
            g.search.clear();
            g.log_path = None;
            g.log_rev = "HEAD".into();
            g.reveal = Some(sha);
        });
        host.show_panel(PANEL);
        self.spawn(async move { ctx.refresh().await });
    }
}

impl GitPlugin {
    /// The blame of the file in front (with what is typed and not saved).
    pub fn blame(&self) {
        let ctx = self.ctx();
        self.spawn(async move { blame_now(&ctx).await });
    }
}

/// The blame of the file in front, shown beside its lines.
pub async fn blame_now(ctx: &Ctx) {
    let Ok(file) = ctx.host.active_file() else {
        return;
    };
    let Some(path) = file.path else {
        return;
    };
    let Some(root) = git::repo_root(&path).await else {
        ctx.host.error(t!("git.file_no_repo"));
        return;
    };
    let Some(inner) = relative(&root, &path) else {
        return;
    };
    let options = ctx.with(|g| g.blame_options.clone());
    match history::blame_contents(&root, &inner, &file.text, &options).await {
        Ok(lines) => {
            ctx.host.annotate(&path, Some(annotations(&lines)));
            ctx.with(|g| {
                g.blame = Some(Blame {
                    path,
                    repo: root,
                    relative: inner,
                    shas: lines.iter().map(|l| l.sha.clone()).collect(),
                })
            });
        }
        Err(error) => ctx.host.error(error),
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    if let Err(error) = run(GitPlugin { ctx: None, runtime }) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(sha: &str, author: &str) -> history::BlameLine {
        history::BlameLine {
            sha: sha.into(),
            author: author.into(),
            email: format!("{author}@example.com"),
            time: 0,
            summary: "a summary".into(),
            message: "a message".into(),
        }
    }

    #[test]
    fn a_blame_is_one_card_per_commit_and_untinted_lines_not_committed() {
        let lines = [
            line("abc", "Ada"),
            line("abc", "Ada"),
            line("def", "Bob"),
            line("0000", "Not Committed Yet"),
        ];
        let notes = annotations(&lines);
        assert_eq!(notes.details.len(), 2);
        assert_eq!(notes.lines[0].detail, Some(0));
        assert_eq!(notes.lines[1].tint, notes.lines[0].tint);
        assert_eq!(notes.lines[2].detail, Some(1));
        assert!(notes.lines[0].text.ends_with("Ada"));
        assert_eq!(notes.lines[3].tint, None);
        assert_eq!(notes.lines[3].detail, None);
        assert!(notes.details[0][0].contains("abc"));
        assert!(notes.details[0].iter().any(|l| l == "a message"));
    }

    #[test]
    fn a_repository_is_made_for_the_project_never_for_the_home_folder() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let inner = project.join("src");
        std::fs::create_dir_all(&inner).unwrap();
        let file = inner.join("main.rs");
        std::fs::write(&file, "").unwrap();
        assert_eq!(init_target(&file, std::slice::from_ref(&project)), Some(project));
        assert_eq!(init_target(&file, &[]), Some(inner));
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(init_target(Path::new(&home), &[]), None);
        }
        assert_eq!(init_target(Path::new("/"), &[]), None);
    }

    #[test]
    fn a_path_inside_a_repository_is_written_as_git_does() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a").join("b.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "").unwrap();
        assert_eq!(relative(dir.path(), &file).as_deref(), Some("a/b.txt"));
        assert_eq!(relative(&dir.path().join("a"), dir.path()), None);
    }
}
