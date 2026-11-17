//! What the Git panel holds: the repository, its state, and what is picked
//! and typed in each tab.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::git::graph::{Commit, GraphRow};
use crate::git::history::{self, BlameOptions, PlanItem, SearchMode, Stash};
use crate::git::tools::{CommitOptions, MergeOptions, PushSpec, RebaseOptions, Remote};
use crate::git::{Change, DiffOptions, MergeMode, PullMode, Source, Status};

/// How many commits the log shows at first, and more each time.
pub const PAGE: usize = 200;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Commit,
    Stash,
    Log,
    Branches,
    Tools,
}

/// The tool shown in the Tools tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tool {
    /// The file in front: compare it with a revision, get it from one.
    File,
    #[default]
    Push,
    Remotes,
    Patch,
    Merge,
    Rebase,
    Clean,
    Trees,
    Clone,
    Repos,
}

impl Tool {
    pub const ALL: [Tool; 10] = [
        Tool::File,
        Tool::Push,
        Tool::Remotes,
        Tool::Patch,
        Tool::Merge,
        Tool::Rebase,
        Tool::Clean,
        Tool::Trees,
        Tool::Clone,
        Tool::Repos,
    ];

    pub fn label(self) -> String {
        match self {
            Tool::File => t!("git.tool_file"),
            Tool::Push => t!("git.push"),
            Tool::Remotes => t!("git.remotes"),
            Tool::Patch => t!("git.patches"),
            Tool::Merge => t!("git.merge_options"),
            Tool::Rebase => t!("git.rebase_options"),
            Tool::Clean => t!("git.clean_title"),
            Tool::Trees => t!("git.trees_title"),
            Tool::Clone => t!("git.clone_title"),
            Tool::Repos => t!("git.repos_title"),
        }
        .to_string()
    }
}

/// What a name (or a message) typed in the Log tab is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// A branch at the commit picked.
    Branch,
    /// A tag at the commit picked (the text: its message).
    Tag,
    /// The new message of the commit picked.
    Reword,
    /// The message of the commits picked, folded into one.
    Squash,
    /// Moving the branch to the commit picked.
    Reset,
    /// The message of a new commit made of files taken out of the commit
    /// picked.
    Extract,
}

/// What can be done to a ref of the Branches tab that needs a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefDialog {
    Create,
    Rename,
    Tag,
}

/// The settings of the plugin (Preferences › Plugins).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// `merge`, `rebase` or `ff-only`.
    #[serde(default = "default_pull")]
    pub pull_mode: String,
    /// `default`, `no-ff`, `ff-only` or `squash`.
    #[serde(default = "default_merge")]
    pub merge_mode: String,
    /// Fetch every so many minutes (0: never).
    #[serde(default = "default_fetch")]
    pub auto_fetch_minutes: u32,
}

fn default_pull() -> String {
    "merge".into()
}

fn default_fetch() -> u32 {
    5
}

fn default_merge() -> String {
    "default".into()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            pull_mode: default_pull(),
            merge_mode: default_merge(),
            auto_fetch_minutes: 5,
        }
    }
}

impl Settings {
    pub fn pull(&self) -> PullMode {
        match self.pull_mode.as_str() {
            "rebase" => PullMode::Rebase,
            "ff-only" => PullMode::FastForwardOnly,
            _ => PullMode::Merge,
        }
    }

    pub fn merge(&self) -> MergeMode {
        match self.merge_mode.as_str() {
            "no-ff" => MergeMode::NoFastForward,
            "ff-only" => MergeMode::FastForwardOnly,
            "squash" => MergeMode::Squash,
            _ => MergeMode::Default,
        }
    }
}

/// A diff tab the plugin shows: its files, and how they are read.
#[derive(Debug, Clone)]
pub struct DiffTab {
    pub title: String,
    pub files: Vec<(Source, Change)>,
    pub options: DiffOptions,
    pub start: usize,
}

/// The blame shown beside a file.
#[derive(Debug, Clone)]
pub struct Blame {
    pub path: PathBuf,
    pub repo: PathBuf,
    /// The file inside the repository.
    pub relative: String,
    /// The commit of each line.
    pub shas: Vec<String>,
}

/// A repository of the projects, for the Repositories tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoLine {
    pub root: PathBuf,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changes: usize,
}

#[derive(Default)]
pub struct Git {
    pub repo: Option<PathBuf>,
    /// A repository was looked for (none found: the panel offers to make one).
    pub searched: bool,
    /// Where "Initialize" makes one.
    pub init_dir: Option<PathBuf>,
    pub workspace: Vec<PathBuf>,
    pub current_file: Option<PathBuf>,
    pub tab: Tab,
    pub status: Option<Status>,
    /// What is under way (the buttons wait).
    pub busy: Option<String>,
    /// The last thing said: the text, and whether it is an error.
    pub notice: Option<(String, bool)>,
    /// A button that asks to be clicked twice, and was clicked once.
    pub armed: Option<String>,
    pub settings: Settings,

    // Commit
    /// The file picked: in the index or not, and its path.
    pub picked: Option<(bool, String)>,
    pub message: String,
    pub amend: bool,
    pub options: CommitOptions,
    pub show_options: bool,
    pub recent: Vec<String>,

    // Stash
    pub stashes: Vec<Stash>,
    pub stash: Option<usize>,
    pub stash_files: Vec<Change>,
    pub stash_message: String,
    pub stash_keep_index: bool,
    pub stash_staged: bool,
    pub stash_branch: String,

    // Log
    pub commits: Vec<Commit>,
    /// The commits of the current branch (the others are drawn hollow).
    pub in_head: std::collections::HashSet<String>,
    pub rows: Vec<GraphRow>,
    pub limit: usize,
    /// The revision whose history is shown (`HEAD`, a branch, `--all`).
    pub log_rev: String,
    pub search: String,
    pub search_mode: SearchMode,
    /// The history of one file only.
    pub log_path: Option<String>,
    /// Clicks pick several commits (to compare, revert, squash…).
    pub several: bool,
    /// The commits picked (the last one is shown).
    pub selected: Vec<String>,
    pub commit_files: Vec<Change>,
    pub commit_message: String,
    /// How the signature of the commit shown checks out.
    pub signature: Option<String>,
    /// The branches holding the commit shown.
    pub containing: Vec<String>,
    /// The commit shown is on a remote: rewriting it needs a force push.
    pub pushed: bool,
    /// A file of the commit shown, picked.
    pub commit_file: Option<String>,
    pub show_filters: bool,
    pub filter: history::LogFilter,
    pub dialog: Option<Dialog>,
    pub dialog_text: String,
    pub reset_mode: String,
    /// An interactive rebase being planned: from which commit, and how.
    pub plan: Option<(Option<String>, Vec<PlanItem>)>,
    /// A commit to pick once the log has loaded (from the blame).
    pub reveal: Option<String>,

    // Branches
    pub refs: history::Refs,
    /// `b:name`, `r:origin/name` or `t:name`.
    pub reference: Option<String>,
    pub ref_dialog: Option<RefDialog>,
    pub ref_name: String,
    pub tag_message: String,
    pub branch_filter: String,

    // Tools
    pub tool: Tool,
    /// A revision typed for the file in front.
    pub file_rev: String,
    pub push: PushSpec,
    /// The commits a push would send.
    pub outgoing: Vec<Commit>,
    pub remotes: Vec<Remote>,
    /// The remote being edited (none: a new one).
    pub remote_edit: Option<String>,
    pub remote_name: String,
    pub remote_url: String,
    pub patch: String,
    pub merge_branch: String,
    pub merge: MergeOptions,
    pub rebase_branch: String,
    pub rebase: RebaseOptions,
    pub clean: Vec<String>,
    pub clean_ignored: bool,
    pub submodules: Vec<crate::git::tools::Submodule>,
    pub worktrees: Vec<crate::git::tools::Worktree>,
    pub clone_url: String,
    pub clone_dest: String,
    pub repos: Vec<RepoLine>,

    // The editor
    pub blame: Option<Blame>,
    pub blame_options: BlameOptions,
    pub diffs: BTreeMap<String, DiffTab>,
}

impl Git {
    pub fn new() -> Self {
        Git {
            limit: PAGE,
            log_rev: "HEAD".into(),
            reset_mode: "mixed".into(),
            ..Git::default()
        }
    }

    /// The commit shown in the Log tab.
    pub fn commit(&self) -> Option<&Commit> {
        let sha = self.selected.last()?;
        self.commits.iter().find(|c| &c.sha == sha)
    }

    /// A file of the Commit tab: its change, from the index or not.
    pub fn change(&self, staged: bool, path: &str) -> Option<&Change> {
        let status = self.status.as_ref()?;
        let list = if staged {
            &status.staged
        } else {
            &status.unstaged
        };
        list.iter().find(|c| c.path == path)
    }

    pub fn conflicts(&self) -> Vec<&Change> {
        self.status.as_ref().map_or_else(Vec::new, |s| {
            s.unstaged
                .iter()
                .filter(|c| c.kind == crate::git::Kind::Conflict)
                .collect()
        })
    }

    /// Says something in the panel.
    pub fn say(&mut self, result: Result<String, String>) {
        self.notice = match result {
            Ok(text) if text.is_empty() => None,
            Ok(text) => Some((text, false)),
            Err(error) if error == crate::git::CONFLICT => {
                Some((t!("git.conflicts_to_settle").to_string(), true))
            }
            Err(error) => Some((error, true)),
        };
    }

    /// A button that acts only on a second click: true on that one.
    pub fn confirm(&mut self, action: &str) -> bool {
        if self.armed.as_deref() == Some(action) {
            self.armed = None;
            true
        } else {
            self.armed = Some(action.to_string());
            false
        }
    }
}

/// `branch ↑2 ↓1`
pub fn branch_label(branch: &str, ahead: u32, behind: u32) -> String {
    let mut label = branch.to_string();
    if ahead > 0 {
        label.push_str(&format!(" ↑{ahead}"));
    }
    if behind > 0 {
        label.push_str(&format!(" ↓{behind}"));
    }
    label
}

/// `2026-09-03`
pub fn format_date(epoch: i64) -> String {
    format(epoch, &t!("git.date_format"))
}

/// `2026-09-03 14:05`
pub fn format_time(epoch: i64) -> String {
    format(epoch, &t!("git.datetime_format"))
}

fn format(epoch: i64, pattern: &str) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(epoch, 0)
        .single()
        .map(|t| t.format(pattern).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_say_how_to_pull_and_merge() {
        let mut s = Settings::default();
        assert_eq!(s.pull(), PullMode::Merge);
        assert_eq!(s.merge(), MergeMode::Default);
        s.pull_mode = "rebase".into();
        s.merge_mode = "squash".into();
        assert_eq!(s.pull(), PullMode::Rebase);
        assert_eq!(s.merge(), MergeMode::Squash);
        // Settings of an older file: what is missing takes its default.
        let old: Settings = serde_json::from_str(r#"{"auto_fetch_minutes": 5}"#).unwrap();
        assert_eq!(old.pull_mode, "merge");
        assert_eq!(old.auto_fetch_minutes, 5);
    }

    #[test]
    fn a_dangerous_button_acts_on_the_second_click_only() {
        let mut g = Git::new();
        assert!(!g.confirm("confirm:x"));
        assert!(g.confirm("confirm:x"));
        assert!(!g.confirm("confirm:x"));
        assert!(!g.confirm("confirm:y"));
        assert!(!g.confirm("confirm:x"), "another button disarms");
    }

    #[test]
    fn what_is_said_tells_errors_and_conflicts() {
        let mut g = Git::new();
        g.say(Ok(String::new()));
        assert_eq!(g.notice, None);
        g.say(Ok("done".into()));
        assert_eq!(g.notice, Some(("done".into(), false)));
        g.say(Err(crate::git::CONFLICT.into()));
        assert_eq!(
            g.notice,
            Some((t!("git.conflicts_to_settle").to_string(), true))
        );
    }

    #[test]
    fn a_branch_says_how_far_it_is_from_its_upstream() {
        assert_eq!(branch_label("main", 0, 0), "main");
        assert_eq!(branch_label("main", 2, 1), "main ↑2 ↓1");
    }
}
