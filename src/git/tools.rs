//! What goes beyond the everyday commands: renaming a branch, the remotes,
//! a push with its options, updating after a refused push, fixups, taking
//! files out of a commit, patches, merges and rebases with options.

use std::path::Path;

use super::graph::Commit;
use super::history::{self, parse_log};
use super::{run, run_push, run_with, stopped, MergeMode, PullMode};

// ─── Branches, files ────────────────────────────────────────────────────────

pub async fn rename_branch(repo: &Path, old: &str, new: &str) -> Result<(), String> {
    if new.is_empty() || new.starts_with('-') {
        return Err(t!("git.err_branch_name").to_string());
    }
    run(repo, &["branch", "-m", old, new]).await.map(|_| ())
}

/// Puts a file as `rev` has it, in the index and on disk.
pub async fn get_from_revision(repo: &Path, rev: &str, path: &str) -> Result<(), String> {
    let source = format!("--source={rev}");
    run(
        repo,
        &["restore", &source, "--staged", "--worktree", "--", path],
    )
    .await
    .map(|_| ())
}

// ─── Cleaning, cloning ──────────────────────────────────────────────────────

/// The files and folders `git clean` would remove (untracked ones, and the
/// ignored ones too with `ignored`), without removing anything.
pub async fn clean_preview(repo: &Path, ignored: bool) -> Result<Vec<String>, String> {
    let mut args = vec!["clean", "-nd"];
    if ignored {
        args.push("-x");
    }
    // The lines are read in English, whatever the user's language.
    let out = run_with(repo, &args, None, &[("LC_ALL", "C")]).await?;
    Ok(out
        .lines()
        .filter_map(|l| l.strip_prefix("Would remove "))
        .map(str::to_string)
        .collect())
}

/// Removes what [`clean_preview`] lists.
pub async fn clean_run(repo: &Path, ignored: bool) -> Result<(), String> {
    let mut args = vec!["clean", "-fd"];
    if ignored {
        args.push("-x");
    }
    run(repo, &args).await.map(|_| ())
}

/// Clones `url` into the folder `dest` (which must not exist yet; its parent
/// must).
pub async fn clone_repo(url: &str, dest: &Path) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') {
        return Err(t!("git.err_clone_url").to_string());
    }
    let (Some(parent), Some(name)) = (dest.parent(), dest.file_name()) else {
        return Err(t!("git.err_clone_dest").to_string());
    };
    if !parent.is_dir() || dest.exists() {
        return Err(t!("git.err_clone_dest").to_string());
    }
    let name = name.to_string_lossy();
    super::run_network(parent, &["clone", "--", url, &name])
        .await
        .map(|_| ())
}

// ─── Submodules, worktrees ──────────────────────────────────────────────────

/// A submodule of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    pub path: String,
    /// What git says of it: `-` not initialised, `+` not at the commit the
    /// repository wants, `U` in conflict, a space when all is well.
    pub state: char,
    pub sha: String,
}

/// The submodules, as `git submodule status` lists them.
pub async fn submodules(repo: &Path) -> Result<Vec<Submodule>, String> {
    let out = run(repo, &["submodule", "status"]).await?;
    Ok(out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let state = line.chars().next()?;
            let rest = line.get(state.len_utf8()..)?;
            let mut words = rest.split_whitespace();
            let sha = words.next()?.to_string();
            let path = words.next()?.to_string();
            Some(Submodule {
                path,
                state: if state == ' ' { ' ' } else { state },
                sha,
            })
        })
        .collect())
}

/// Initialises and updates every submodule, nested ones too.
pub async fn submodules_update(repo: &Path) -> Result<(), String> {
    super::run_network(repo, &["submodule", "update", "--init", "--recursive"])
        .await
        .map(|_| ())
}

/// Adds a repository as a submodule, in the folder `path` of this one.
pub async fn submodule_add(repo: &Path, url: &str, path: &str) -> Result<(), String> {
    let (url, path) = (url.trim(), path.trim());
    if url.is_empty() || url.starts_with('-') {
        return Err(t!("git.err_clone_url").to_string());
    }
    if path.is_empty() || path.starts_with('-') {
        return Err(t!("git.err_submodule_path").to_string());
    }
    super::run_network(repo, &["submodule", "add", "--", url, path])
        .await
        .map(|_| ())
}

/// Makes a worktree in `folder`: on a new branch `branch` made where HEAD
/// is, or on the branch (or commit) `branch` that exists.
pub async fn worktree_add(
    repo: &Path,
    folder: &str,
    branch: &str,
    new_branch: bool,
) -> Result<(), String> {
    let (folder, branch) = (folder.trim(), branch.trim());
    if folder.is_empty() || folder.starts_with('-') {
        return Err(t!("git.err_worktree_folder").to_string());
    }
    if branch.is_empty() || branch.starts_with('-') {
        return Err(t!("git.err_branch_name").to_string());
    }
    let args: Vec<&str> = if new_branch {
        vec!["worktree", "add", "-b", branch, "--", folder]
    } else {
        vec!["worktree", "add", "--", folder, branch]
    };
    run(repo, &args).await.map(|_| ())
}

/// Removes a worktree and its folder. Without `force`, git keeps one that
/// holds changes or files it does not know.
pub async fn worktree_remove(repo: &Path, folder: &Path, force: bool) -> Result<(), String> {
    let folder = folder.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", folder.as_ref()]);
    run(repo, &args).await.map(|_| ())
}

// ─── Bisect ─────────────────────────────────────────────────────────────────

/// What is said of the commit a bisect has checked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Good,
    Bad,
    /// It cannot be tried: another one near it is taken.
    Skip,
}

/// A search for the commit that brought a fault, as it stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bisect {
    /// The commit to try: its short name and its subject.
    pub current: String,
    /// The first bad commit, once found (its full name).
    pub found: Option<String>,
    /// What git last said (how many commits are left).
    pub said: String,
}

/// git's words are read here: they must not be translated.
async fn bisect_run(repo: &Path, args: &[&str]) -> Result<Bisect, String> {
    let said = run_with(repo, args, None, &[("LC_ALL", "C")]).await?;
    let found = said
        .lines()
        .find_map(|l| l.strip_suffix(" is the first bad commit"))
        .map(str::to_string);
    let current = run(repo, &["log", "-1", "--format=%h %s"]).await?;
    Ok(Bisect {
        current: current.trim().to_string(),
        found,
        said: said.lines().next().unwrap_or_default().to_string(),
    })
}

/// Whether a bisect is under way.
pub async fn bisecting(repo: &Path) -> bool {
    match run(repo, &["rev-parse", "--absolute-git-dir"]).await {
        Ok(dir) => Path::new(dir.trim()).join("BISECT_START").exists(),
        Err(_) => false,
    }
}

/// The commit a bisect under way has checked out.
pub async fn bisect_current(repo: &Path) -> Option<Bisect> {
    if !bisecting(repo).await {
        return None;
    }
    let current = run(repo, &["log", "-1", "--format=%h %s"]).await.ok()?;
    Some(Bisect {
        current: current.trim().to_string(),
        ..Bisect::default()
    })
}

/// Starts looking between a revision that has the fault and one that does
/// not. What is not committed must be put aside first: the search checks
/// commits out.
pub async fn bisect_start(repo: &Path, bad: &str, good: &str) -> Result<Bisect, String> {
    super::ensure_idle(repo).await?;
    if bisecting(repo).await {
        return Err(t!("git.err_bisecting").to_string());
    }
    let changed = run(repo, &["status", "--porcelain", "--untracked-files=no"]).await?;
    if !changed.trim().is_empty() {
        return Err(t!("git.err_bisect_dirty").to_string());
    }
    let bad = bad.trim();
    let bad = if bad.is_empty() { "HEAD" } else { bad };
    let bad = super::verify_rev(repo, bad).await?;
    let good = super::verify_rev(repo, good.trim()).await?;
    let started = bisect_run(repo, &["bisect", "start", &bad, &good, "--"]).await;
    if started.is_err() && bisecting(repo).await {
        // Refused (the revisions the wrong way round): nothing is left over.
        let _ = run(repo, &["bisect", "reset"]).await;
    }
    started
}

pub async fn bisect_mark(repo: &Path, verdict: Verdict) -> Result<Bisect, String> {
    let word = match verdict {
        Verdict::Good => "good",
        Verdict::Bad => "bad",
        Verdict::Skip => "skip",
    };
    bisect_run(repo, &["bisect", word]).await
}

/// Ends the search and goes back where it started from.
pub async fn bisect_stop(repo: &Path) -> Result<(), String> {
    run(repo, &["bisect", "reset"]).await.map(|_| ())
}

// ─── Identity ───────────────────────────────────────────────────────────────

/// Who commits in a repository, and how the commits are signed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Identity {
    /// What a commit made now would carry.
    pub name: String,
    pub email: String,
    /// The name or the email is the repository's own.
    pub own: bool,
    pub signing_key: String,
    /// Every commit of the repository is signed.
    pub sign_all: bool,
}

async fn config(repo: &Path, args: &[&str]) -> String {
    let mut all = vec!["config"];
    all.extend(args);
    // A key that is not set is an exit status, not a failure.
    run(repo, &all)
        .await
        .map(|v| v.trim().to_string())
        .unwrap_or_default()
}

pub async fn identity(repo: &Path) -> Identity {
    Identity {
        name: config(repo, &["--get", "user.name"]).await,
        email: config(repo, &["--get", "user.email"]).await,
        own: !config(repo, &["--local", "--get", "user.name"])
            .await
            .is_empty()
            || !config(repo, &["--local", "--get", "user.email"])
                .await
                .is_empty(),
        signing_key: config(repo, &["--get", "user.signingkey"]).await,
        sign_all: config(repo, &["--type=bool", "--get", "commit.gpgsign"]).await == "true",
    }
}

/// Gives the repository an identity of its own. The global one is not
/// touched.
pub async fn set_identity(repo: &Path, name: &str, email: &str) -> Result<(), String> {
    let (name, email) = (name.trim(), email.trim());
    if name.is_empty() || !email.contains('@') {
        return Err(t!("git.err_identity").to_string());
    }
    run(repo, &["config", "--local", "user.name", name]).await?;
    run(repo, &["config", "--local", "user.email", email])
        .await
        .map(|_| ())
}

/// Takes a key out of the repository's own settings, if it is there.
async fn unset_local(repo: &Path, key: &str) -> Result<(), String> {
    if config(repo, &["--local", "--get", key]).await.is_empty() {
        return Ok(());
    }
    run(repo, &["config", "--local", "--unset-all", key])
        .await
        .map(|_| ())
}

/// Goes back to the global identity.
pub async fn clear_identity(repo: &Path) -> Result<(), String> {
    unset_local(repo, "user.name").await?;
    unset_local(repo, "user.email").await
}

/// The key the commits of the repository are signed with (empty: the one of
/// the global settings).
pub async fn set_signing_key(repo: &Path, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return unset_local(repo, "user.signingkey").await;
    }
    run(repo, &["config", "--local", "user.signingkey", key])
        .await
        .map(|_| ())
}

/// Whether every commit of the repository is signed.
pub async fn set_sign_all(repo: &Path, on: bool) -> Result<(), String> {
    let value = if on { "true" } else { "false" };
    run(repo, &["config", "--local", "commit.gpgsign", value])
        .await
        .map(|_| ())
}

/// A worktree of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: std::path::PathBuf,
    /// The branch, or `HEAD detached`.
    pub branch: String,
}

/// The worktrees, the main one first.
pub async fn worktrees(repo: &Path) -> Result<Vec<Worktree>, String> {
    let out = run(repo, &["worktree", "list", "--porcelain"]).await?;
    let mut found: Vec<Worktree> = Vec::new();
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            found.push(Worktree {
                path: path.into(),
                branch: "HEAD detached".to_string(),
            });
        } else if let (Some(branch), Some(last)) = (line.strip_prefix("branch "), found.last_mut())
        {
            last.branch = branch.trim_start_matches("refs/heads/").to_string();
        } else if line == "bare" {
            if let Some(last) = found.last_mut() {
                last.branch = "bare".to_string();
            }
        }
    }
    Ok(found)
}

// ─── Committing ─────────────────────────────────────────────────────────────

/// What a commit can be made with, besides its message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitOptions {
    /// `Name <email>`, when someone else wrote it.
    pub author: String,
    pub signoff: bool,
    /// The hooks are not run.
    pub no_verify: bool,
    /// The commit is signed (GPG or SSH, as git is set up).
    pub sign: bool,
}

impl CommitOptions {
    /// What is added to `git commit`.
    pub fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let author = self.author.trim();
        if !author.is_empty() {
            args.push(format!("--author={author}"));
        }
        if self.signoff {
            args.push("--signoff".to_string());
        }
        if self.no_verify {
            args.push("--no-verify".to_string());
        }
        if self.sign {
            args.push("--gpg-sign".to_string());
        }
        args
    }
}

/// The last messages of the branch, latest first, each once.
pub async fn recent_messages(repo: &Path, limit: usize) -> Result<Vec<String>, String> {
    let max = format!("--max-count={}", limit * 3);
    let out = run(repo, &["log", &max, "--format=%B%x1e"]).await?;
    let mut messages: Vec<String> = Vec::new();
    for message in out.split('\x1e').map(str::trim).filter(|m| !m.is_empty()) {
        if !messages.iter().any(|m| m == message) {
            messages.push(message.to_string());
        }
    }
    messages.truncate(limit);
    Ok(messages)
}

// ─── Remotes ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

pub async fn remotes(repo: &Path) -> Result<Vec<Remote>, String> {
    Ok(parse_remotes(&run(repo, &["remote", "-v"]).await?))
}

/// `git remote -v`: two lines by remote, its fetch one is kept.
pub fn parse_remotes(out: &str) -> Vec<Remote> {
    out.lines()
        .filter(|l| l.ends_with("(fetch)"))
        .filter_map(|l| {
            let (name, rest) = l.split_once('\t')?;
            let url = rest.strip_suffix("(fetch)")?.trim();
            Some(Remote {
                name: name.to_string(),
                url: url.to_string(),
            })
        })
        .collect()
}

pub async fn add_remote(repo: &Path, name: &str, url: &str) -> Result<(), String> {
    run(repo, &["remote", "add", "--", name, url])
        .await
        .map(|_| ())
}

/// A new name and address for a remote.
pub async fn edit_remote(repo: &Path, old: &str, name: &str, url: &str) -> Result<(), String> {
    if old != name {
        run(repo, &["remote", "rename", "--", old, name]).await?;
    }
    run(repo, &["remote", "set-url", "--", name, url])
        .await
        .map(|_| ())
}

pub async fn remove_remote(repo: &Path, name: &str) -> Result<(), String> {
    run(repo, &["remote", "remove", "--", name])
        .await
        .map(|_| ())
}

// ─── Pushing ────────────────────────────────────────────────────────────────

/// Which tags go with a push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PushTags {
    #[default]
    None,
    /// Those of the commits that are pushed (`--follow-tags`).
    Current,
    /// All of them (`--tags`).
    All,
}

impl PushTags {
    pub const ALL: [PushTags; 3] = [PushTags::None, PushTags::Current, PushTags::All];
}

impl std::fmt::Display for PushTags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            PushTags::None => t!("git.push_tags_none"),
            PushTags::Current => t!("git.push_tags_current"),
            PushTags::All => t!("git.push_tags_all"),
        })
    }
}

/// A push as the dialog sets it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushSpec {
    pub remote: String,
    /// What is sent: the branch, or a commit (up to there).
    pub source: String,
    /// The branch it goes to on the remote.
    pub target: String,
    pub set_upstream: bool,
    pub tags: PushTags,
    pub force: bool,
}

impl PushSpec {
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["push".to_string()];
        if self.force {
            args.push("--force-with-lease".to_string());
            args.push(super::FORCE_IF_INCLUDES.to_string());
        }
        match self.tags {
            PushTags::None => {}
            PushTags::Current => args.push("--follow-tags".to_string()),
            PushTags::All => args.push("--tags".to_string()),
        }
        if self.set_upstream {
            args.push("--set-upstream".to_string());
        }
        args.push("--".to_string());
        args.push(self.remote.clone());
        args.push(format!("{}:refs/heads/{}", self.source, self.target));
        args
    }
}

pub async fn push(repo: &Path, spec: &PushSpec) -> Result<(), String> {
    let args = spec.args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_push(repo, &args).await.map(|_| ())
}

/// The commits a push would send: those the target lacks, or, with no
/// target yet, those no remote has.
pub async fn outgoing(
    repo: &Path,
    source: &str,
    target: Option<&str>,
) -> Result<Vec<Commit>, String> {
    let format = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%D%x1e";
    let range;
    let mut args = vec!["log", "--max-count=500", format];
    match target {
        Some(target) => {
            range = format!("{target}..{source}");
            args.extend(["--end-of-options", range.as_str(), "--"]);
        }
        // `--not` can't follow `--end-of-options`.
        None if source.starts_with('-') => return Err(t!("git.err_branch_name").to_string()),
        None => args.extend([source, "--not", "--remotes", "--"]),
    }
    Ok(parse_log(&run(repo, &args).await?))
}

/// Whether a push failed because the remote has something more.
pub fn is_rejected(error: &str) -> bool {
    error.contains("[rejected]")
        || error.contains("non-fast-forward")
        || error.contains("fetch first")
}

/// After a refused push: what the remote has, then the push again.
pub async fn update_and_push(repo: &Path, spec: &PushSpec, mode: PullMode) -> Result<(), String> {
    super::pull(repo, mode, Some((&spec.remote, &spec.target))).await?;
    push(repo, spec).await
}

// ─── Fixups, files of a commit ──────────────────────────────────────────────

/// Adds what is staged to a commit of the branch: a fixup drops the new
/// message; `squash` (nothing to write) keeps the commit's as it is too.
pub async fn fixup(repo: &Path, sha: &str, squash: bool) -> Result<(), String> {
    super::ensure_idle(repo).await?;
    let staged = run(repo, &["diff", "--cached", "--name-only"]).await?;
    if staged.trim().is_empty() {
        return Err(t!("git.err_nothing_staged").to_string());
    }
    let base = history::parent_of(repo, sha).await?;
    history::ensure_linear(repo, base.as_deref()).await?;
    let how = format!("--{}={sha}", if squash { "squash" } else { "fixup" });
    let quiet = [("GIT_EDITOR", "true")];
    run_with(repo, &["commit", "-q", "--no-verify", &how], None, &quiet).await?;
    let mut args = vec!["rebase", "-i", "--autosquash", "--autostash"];
    match base.as_deref() {
        Some(base) => args.push(base),
        None => args.push("--root"),
    }
    let env = [("GIT_SEQUENCE_EDITOR", "true"), ("GIT_EDITOR", "true")];
    match run_with(repo, &args, None, &env).await {
        Ok(_) => Ok(()),
        Err(e) => Err(stopped(repo, e, &["rebase", "--abort"]).await),
    }
}

/// How many files a commit changes.
async fn file_count(repo: &Path, sha: &str) -> Result<usize, String> {
    let out = run(
        repo,
        &["show", "--format=", "--name-only", "--no-renames", sha],
    )
    .await?;
    Ok(out.lines().filter(|l| !l.is_empty()).count())
}

/// Replays the branch from `sha` on, running `script` right after it.
async fn rewrite_commit(
    repo: &Path,
    sha: &str,
    paths: &[String],
    script: String,
    message: Option<&str>,
) -> Result<(), String> {
    let Some(base) = history::parent_of(repo, sha).await? else {
        return Err(t!("git.err_root_commit").to_string());
    };
    history::ensure_linear(repo, Some(&base)).await?;
    if file_count(repo, sha).await? <= paths.len() {
        return Err(t!("git.err_all_files").to_string());
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let write = |name: &str, content: &[u8]| {
        let path = dir.path().join(name);
        std::fs::write(&path, content)
            .map(|_| path)
            .map_err(|e| e.to_string())
    };
    let mut files = vec![
        (
            "SE_PATHS".to_string(),
            write("paths", paths.join("\0").as_bytes())?,
        ),
        ("SE_SCRIPT".to_string(), write("script", script.as_bytes())?),
    ];
    if let Some(message) = message {
        files.push((
            "SE_MESSAGE".to_string(),
            write("message", message.as_bytes())?,
        ));
    }
    let mut todo = String::new();
    for commit in history::commits_after(repo, Some(&base)).await? {
        todo.push_str(&format!("pick {commit}\n"));
        if commit == sha {
            todo.push_str("exec sh \"$SE_SCRIPT\"\n");
        }
    }
    history::rebase_todo(repo, Some(&base), &todo, &files).await
}

const PATHS: &str = "--pathspec-from-file=\"$SE_PATHS\" --pathspec-file-nul";

/// Takes files out of a commit, as if it had not touched them.
pub async fn remove_from_commit(repo: &Path, sha: &str, paths: &[String]) -> Result<(), String> {
    let script = format!(
        "set -e\ngit restore --source=HEAD~ --staged --worktree {PATHS}\n\
         git commit --amend -q --no-edit --no-verify\n"
    );
    rewrite_commit(repo, sha, paths, script, None).await
}

/// Moves what a commit does to some files into a commit of their own, right
/// after it.
pub async fn extract_from_commit(
    repo: &Path,
    sha: &str,
    paths: &[String],
    message: &str,
) -> Result<(), String> {
    let script = format!(
        "set -e\ngit restore --source=HEAD~ --staged --worktree {PATHS}\n\
         git commit --amend -q --no-edit --no-verify\n\
         git restore --source={sha} --staged --worktree {PATHS}\n\
         git commit -q --no-verify -F \"$SE_MESSAGE\"\n"
    );
    rewrite_commit(repo, sha, paths, script, Some(message)).await
}

/// Applies what a commit did to some files, without committing.
pub async fn apply_from_commit(repo: &Path, sha: &str, paths: &[String]) -> Result<(), String> {
    let mut args = vec!["show", "--format=", "--binary", sha, "--"];
    args.extend(paths.iter().map(String::as_str));
    let patch = run(repo, &args).await?;
    run_with(
        repo,
        &["apply", "--3way", "--whitespace=nowarn"],
        Some(&patch),
        &[],
    )
    .await
    .map(|_| ())
}

// ─── Patches ────────────────────────────────────────────────────────────────

/// What is not committed, as a patch.
pub async fn local_patch(repo: &Path) -> Result<String, String> {
    run(repo, &["diff", "--binary", "HEAD"]).await
}

/// Commits (oldest first), as patches that keep their author and message.
pub async fn commits_patch(repo: &Path, shas: &[String]) -> Result<String, String> {
    let mut patch = String::new();
    for sha in shas {
        let one = run(repo, &["format-patch", "--stdout", "--binary", "-1", sha]).await?;
        patch.push_str(&one);
    }
    Ok(patch)
}

/// Whether a patch comes from `git format-patch` (it then makes commits).
pub fn is_mail_patch(patch: &str) -> bool {
    patch.starts_with("From ") && patch.contains("\nSubject:")
}

pub async fn apply_patch(repo: &Path, patch: &str) -> Result<(), String> {
    if patch.trim().is_empty() {
        return Err(t!("git.err_empty_patch").to_string());
    }
    if is_mail_patch(patch) {
        // `am --abort` would also end an `am` or a rebase left half done.
        super::ensure_idle(repo).await?;
        return match run_with(repo, &["am", "--3way"], Some(patch), &[]).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let _ = run(repo, &["am", "--abort"]).await;
                Err(e)
            }
        };
    }
    let plain = ["apply", "--whitespace=nowarn"];
    if run_with(repo, &plain, Some(patch), &[]).await.is_ok() {
        return Ok(());
    }
    run_with(
        repo,
        &["apply", "--3way", "--whitespace=nowarn"],
        Some(patch),
        &[],
    )
    .await
    .map(|_| ())
}

// ─── Merging and rebasing with options ──────────────────────────────────────

/// A merge as its dialog sets it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeOptions {
    pub mode: MergeMode,
    pub no_commit: bool,
    pub no_verify: bool,
    pub allow_unrelated: bool,
    /// The message of the merge commit (empty: git's).
    pub message: String,
}

impl MergeOptions {
    pub fn args(&self, branch: &str) -> Vec<String> {
        let mut args = vec!["merge".to_string()];
        if self.message.trim().is_empty() {
            args.push("--no-edit".to_string());
        } else {
            args.push(format!("--message={}", self.message.trim()));
        }
        args.extend(self.mode.flag().map(str::to_string));
        if self.no_commit {
            args.push("--no-commit".to_string());
        }
        if self.no_verify {
            args.push("--no-verify".to_string());
        }
        if self.allow_unrelated {
            args.push("--allow-unrelated-histories".to_string());
        }
        args.push(branch.to_string());
        args
    }
}

pub async fn merge(repo: &Path, branch: &str, options: &MergeOptions) -> Result<(), String> {
    super::ensure_idle(repo).await?;
    let args = options.args(branch);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(repo, &args).await {
        Ok(_) => Ok(()),
        Err(e) => Err(stopped(repo, e, &["merge", "--abort"]).await),
    }
}

/// A rebase as its dialog sets it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RebaseOptions {
    /// `--onto`: the new base, the branch being the old one.
    pub onto: String,
    pub rebase_merges: bool,
    pub keep_empty: bool,
    pub root: bool,
    pub update_refs: bool,
    pub autosquash: bool,
}

impl RebaseOptions {
    pub fn args(&self, branch: &str) -> Vec<String> {
        let mut args = vec!["rebase".to_string()];
        // `--autosquash` only works in an interactive rebase.
        if self.autosquash {
            args.push("-i".to_string());
            args.push("--autosquash".to_string());
        }
        if self.rebase_merges {
            args.push("--rebase-merges".to_string());
        }
        if self.keep_empty {
            args.push("--keep-empty".to_string());
        }
        if self.update_refs {
            args.push("--update-refs".to_string());
        }
        if !self.onto.trim().is_empty() {
            args.push(format!("--onto={}", self.onto.trim()));
        }
        if self.root {
            args.push("--root".to_string());
        } else {
            args.push(branch.to_string());
        }
        args
    }
}

pub async fn rebase(repo: &Path, branch: &str, options: &RebaseOptions) -> Result<(), String> {
    super::ensure_idle(repo).await?;
    let args = options.args(branch);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let env = [("GIT_SEQUENCE_EDITOR", "true"), ("GIT_EDITOR", "true")];
    match run_with(repo, &args, None, &env).await {
        Ok(_) => Ok(()),
        Err(e) => Err(stopped(repo, e, &["rebase", "--abort"]).await),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    fn write(repo: &Repo, name: &str, text: &str) {
        std::fs::write(repo.path().join(name), text).unwrap();
    }

    fn block<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn a_worktree_is_made_and_removed_and_one_with_work_is_kept() {
        let repo = Repo::new();
        commit(&repo, "a.txt", "a\n", "first");
        let outside = tempfile::tempdir().unwrap();
        let folder = outside.path().join("wt");
        let name = folder.to_string_lossy().into_owned();
        block(worktree_add(repo.path(), &name, "feature", true)).unwrap();
        assert!(block(worktrees(repo.path()))
            .unwrap()
            .iter()
            .any(|w| w.branch == "feature"));
        // With work that is not committed, it is not removed unless forced.
        std::fs::write(folder.join("a.txt"), "changed\n").unwrap();
        assert!(block(worktree_remove(repo.path(), &folder, false)).is_err());
        assert!(folder.join("a.txt").exists());
        block(worktree_remove(repo.path(), &folder, true)).unwrap();
        assert!(!folder.exists());
        assert_eq!(block(worktrees(repo.path())).unwrap().len(), 1);
        // On a branch that exists.
        block(worktree_add(repo.path(), &name, "feature", false)).unwrap();
        assert!(folder.exists());
        assert!(block(worktree_add(repo.path(), "-x", "feature", false)).is_err());
        assert!(block(worktree_add(repo.path(), &name, "--detach", false)).is_err());
    }

    #[test]
    fn a_bisect_finds_the_commit_that_brought_the_fault() {
        let repo = Repo::new();
        for n in 1..=8 {
            let state = if n >= 6 { "broken" } else { "works" };
            commit(
                &repo,
                "state.txt",
                &format!("{state} {n}\n"),
                &format!("c{n}"),
            );
        }
        let bad_one = repo.git(&["rev-parse", "HEAD~2"]).trim().to_string();
        let first = repo.git(&["rev-list", "--max-parents=0", "HEAD"]);
        // The wrong way round: refused, and nothing is left under way.
        assert!(block(bisect_start(repo.path(), first.trim(), "HEAD")).is_err());
        assert!(!block(bisecting(repo.path())));
        // Work not committed would be in the way.
        std::fs::write(repo.path().join("state.txt"), "mine\n").unwrap();
        assert!(block(bisect_start(repo.path(), "HEAD", first.trim())).is_err());
        repo.git(&["checkout", "-q", "--", "state.txt"]);

        let mut step = block(bisect_start(repo.path(), "", first.trim())).unwrap();
        for _ in 0..8 {
            if step.found.is_some() {
                break;
            }
            let text = std::fs::read_to_string(repo.path().join("state.txt")).unwrap();
            let verdict = if text.starts_with("broken") {
                Verdict::Bad
            } else {
                Verdict::Good
            };
            step = block(bisect_mark(repo.path(), verdict)).unwrap();
        }
        assert_eq!(step.found, Some(bad_one));
        block(bisect_stop(repo.path())).unwrap();
        assert!(!block(bisecting(repo.path())));
        assert_eq!(repo.git(&["branch", "--show-current"]).trim(), "main");
    }

    #[test]
    fn a_repository_is_given_an_identity_of_its_own_and_loses_it() {
        let repo = Repo::new();
        block(set_identity(repo.path(), " Ada ", "ada@example.com")).unwrap();
        let who = block(identity(repo.path()));
        assert_eq!(
            (who.name.as_str(), who.email.as_str()),
            ("Ada", "ada@example.com")
        );
        assert!(who.own);
        assert!(block(set_identity(repo.path(), "", "ada@example.com")).is_err());
        assert!(block(set_identity(repo.path(), "Ada", "no-at-sign")).is_err());
        block(set_signing_key(repo.path(), "ABCD1234")).unwrap();
        block(set_sign_all(repo.path(), true)).unwrap();
        let who = block(identity(repo.path()));
        assert_eq!((who.signing_key.as_str(), who.sign_all), ("ABCD1234", true));
        block(set_signing_key(repo.path(), "")).unwrap();
        block(set_sign_all(repo.path(), false)).unwrap();
        block(clear_identity(repo.path())).unwrap();
        // Twice is not a failure.
        block(clear_identity(repo.path())).unwrap();
        let who = block(identity(repo.path()));
        assert!(!who.own && !who.sign_all);
    }

    #[test]
    fn a_signed_commit_is_asked_of_git() {
        let options = CommitOptions {
            sign: true,
            ..CommitOptions::default()
        };
        assert_eq!(options.args(), ["--gpg-sign"]);
        assert!(CommitOptions::default().args().is_empty());
    }

    #[test]
    fn a_submodule_is_added() {
        let source = Repo::new();
        commit(&source, "lib.txt", "lib\n", "lib");
        let repo = Repo::new();
        commit(&repo, "a.txt", "a\n", "first");
        // git refuses a submodule from a folder unless told it may.
        std::env::set_var("GIT_ALLOW_PROTOCOL", "file");
        block(submodule_add(
            repo.path(),
            &source.path().to_string_lossy(),
            "libs/one",
        ))
        .unwrap();
        assert!(repo.path().join("libs/one/lib.txt").exists());
        let listed = block(submodules(repo.path())).unwrap();
        assert!(listed.iter().any(|s| s.path == "libs/one"));
        assert!(block(submodule_add(repo.path(), "-x", "p")).is_err());
        assert!(block(submodule_add(repo.path(), "url", "")).is_err());
    }

    #[test]
    fn clean_lists_what_it_would_remove_and_then_removes_it() {
        let repo = Repo::new();
        commit(&repo, "kept.txt", "k\n", "first");
        write(&repo, ".gitignore", "*.log\n");
        repo.git(&["add", "."]);
        repo.git(&["commit", "-q", "-m", "ignore"]);
        write(&repo, "stray.txt", "x\n");
        write(&repo, "debug.log", "x\n");
        std::fs::create_dir(repo.path().join("scratch")).unwrap();
        write(&repo, "scratch/a.txt", "x\n");
        let listed = block(clean_preview(repo.path(), false)).unwrap();
        assert_eq!(listed, vec!["scratch/", "stray.txt"]);
        let with_ignored = block(clean_preview(repo.path(), true)).unwrap();
        assert!(with_ignored.contains(&"debug.log".to_string()));
        // Looking removed nothing; doing it keeps what git tracks.
        assert!(repo.path().join("stray.txt").exists());
        block(clean_run(repo.path(), false)).unwrap();
        assert!(!repo.path().join("stray.txt").exists());
        assert!(!repo.path().join("scratch").exists());
        assert!(repo.path().join("debug.log").exists());
        assert!(repo.path().join("kept.txt").exists());
    }

    #[test]
    fn a_repository_is_cloned_into_a_new_folder_and_bad_asks_are_refused() {
        let origin = Repo::new();
        commit(&origin, "a.txt", "a\n", "first");
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("copy");
        let url = origin.path().to_string_lossy().into_owned();
        block(clone_repo(&url, &dest)).unwrap();
        assert!(dest.join("a.txt").exists());
        // The folder exists now; an option is no address.
        assert!(block(clone_repo(&url, &dest)).is_err());
        assert!(block(clone_repo("--upload-pack=x", &parent.path().join("o"))).is_err());
        assert!(block(clone_repo("", &parent.path().join("p"))).is_err());
    }

    #[test]
    fn submodules_and_worktrees_are_listed() {
        let lib = Repo::new();
        commit(&lib, "l.txt", "l\n", "lib");
        let repo = Repo::new();
        commit(&repo, "a.txt", "a\n", "first");
        let url = lib.path().to_string_lossy().into_owned();
        // `file` transport is refused by default for submodules.
        repo.git(&[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &url,
            "vendor",
        ]);
        repo.git(&["commit", "-q", "-m", "add the submodule"]);
        let modules = block(submodules(repo.path())).unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(
            (modules[0].path.as_str(), modules[0].state),
            ("vendor", ' ')
        );
        let side = tempfile::tempdir().unwrap();
        let dest = side.path().join("wt");
        repo.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            &dest.to_string_lossy(),
        ]);
        let trees = block(worktrees(repo.path())).unwrap();
        assert_eq!(trees.len(), 2);
        assert_eq!(trees[1].branch, "other");
    }

    fn commit(repo: &Repo, name: &str, text: &str, message: &str) -> String {
        write(repo, name, text);
        repo.git(&["add", "."]);
        repo.git(&["commit", "-q", "-m", message]);
        repo.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn subjects(repo: &Repo) -> Vec<String> {
        repo.git(&["log", "--format=%s"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn read(repo: &Repo, name: &str) -> String {
        std::fs::read_to_string(repo.path().join(name)).unwrap()
    }

    #[tokio::test]
    async fn a_branch_is_renamed_and_a_bad_name_refused() {
        let repo = Repo::new();
        commit(&repo, "a", "1", "one");
        repo.git(&["branch", "old"]);
        rename_branch(repo.path(), "old", "new").await.unwrap();
        assert!(repo.git(&["branch"]).contains("new"));
        assert!(rename_branch(repo.path(), "new", "-x").await.is_err());
    }

    #[tokio::test]
    async fn a_file_comes_back_as_a_revision_has_it() {
        let repo = Repo::new();
        let first = commit(&repo, "a", "1", "one");
        commit(&repo, "a", "2", "two");
        get_from_revision(repo.path(), &first, "a").await.unwrap();
        assert_eq!(read(&repo, "a"), "1");
        assert!(repo.git(&["status", "--porcelain"]).starts_with("M "));
    }

    #[test]
    fn commit_options_make_their_flags() {
        let options = CommitOptions {
            author: " Me <me@x> ".into(),
            signoff: true,
            no_verify: true,
            sign: false,
        };
        assert_eq!(
            options.args(),
            ["--author=Me <me@x>", "--signoff", "--no-verify"]
        );
        assert!(CommitOptions::default().args().is_empty());
    }

    #[tokio::test]
    async fn the_last_messages_come_once_each() {
        let repo = Repo::new();
        commit(&repo, "a", "1", "same");
        commit(&repo, "a", "2", "other\n\nbody");
        commit(&repo, "a", "3", "same");
        let found = recent_messages(repo.path(), 5).await.unwrap();
        assert_eq!(found, ["same", "other\n\nbody"]);
    }

    #[tokio::test]
    async fn remotes_are_added_edited_and_removed() {
        let repo = Repo::new();
        add_remote(repo.path(), "up", "https://a.example/x.git")
            .await
            .unwrap();
        let listed = remotes(repo.path()).await.unwrap();
        assert_eq!(listed[0].url, "https://a.example/x.git");
        edit_remote(repo.path(), "up", "down", "https://b.example/y.git")
            .await
            .unwrap();
        let listed = remotes(repo.path()).await.unwrap();
        assert_eq!(listed[0].name, "down");
        assert_eq!(listed[0].url, "https://b.example/y.git");
        remove_remote(repo.path(), "down").await.unwrap();
        assert!(remotes(repo.path()).await.unwrap().is_empty());
    }

    /// A repository with a bare remote, its `main` pushed.
    fn with_remote() -> (Repo, tempfile::TempDir) {
        let bare = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(bare.path())
            .output()
            .unwrap();
        let repo = Repo::new();
        commit(&repo, "a", "1\n", "one");
        repo.git(&["remote", "add", "origin", bare.path().to_str().unwrap()]);
        repo.git(&["push", "-q", "-u", "origin", "main"]);
        (repo, bare)
    }

    fn spec() -> PushSpec {
        PushSpec {
            remote: "origin".into(),
            source: "main".into(),
            target: "main".into(),
            ..PushSpec::default()
        }
    }

    #[test]
    fn a_push_makes_its_arguments() {
        let spec = PushSpec {
            tags: PushTags::All,
            force: true,
            set_upstream: true,
            ..spec()
        };
        assert_eq!(
            spec.args(),
            [
                "push",
                "--force-with-lease",
                "--force-if-includes",
                "--tags",
                "--set-upstream",
                "--",
                "origin",
                "main:refs/heads/main"
            ]
        );
    }

    #[tokio::test]
    async fn outgoing_commits_and_a_push_up_to_one_of_them() {
        let (repo, _bare) = with_remote();
        let two = commit(&repo, "a", "2\n", "two");
        commit(&repo, "a", "3\n", "three");
        let out = outgoing(repo.path(), "main", Some("origin/main"))
            .await
            .unwrap();
        assert_eq!(out.len(), 2);
        // Up to the older one only.
        let partial = PushSpec {
            source: two.clone(),
            ..spec()
        };
        push(repo.path(), &partial).await.unwrap();
        repo.git(&["fetch", "-q"]);
        assert_eq!(repo.git(&["rev-parse", "origin/main"]).trim(), two.as_str());
        // No upstream: what no remote has.
        repo.git(&["checkout", "-q", "-b", "topic"]);
        let out = outgoing(repo.path(), "topic", None).await.unwrap();
        assert_eq!(out.len(), 1);
    }

    #[tokio::test]
    async fn a_refused_push_is_recognised_and_updated_then_pushed() {
        let (repo, bare) = with_remote();
        // Someone else pushes.
        let other = Repo::new();
        other.git(&["remote", "add", "origin", bare.path().to_str().unwrap()]);
        other.git(&["fetch", "-q", "origin"]);
        other.git(&["checkout", "-q", "-B", "main", "origin/main"]);
        commit(&other, "b", "theirs\n", "theirs");
        other.git(&["push", "-q", "origin", "main"]);
        commit(&repo, "c", "mine\n", "mine");
        let error = push(repo.path(), &spec()).await.unwrap_err();
        assert!(is_rejected(&error), "{error}");
        update_and_push(repo.path(), &spec(), PullMode::Rebase)
            .await
            .unwrap();
        assert_eq!(subjects(&repo), ["mine", "theirs", "one"]);
        assert!(repo.git(&["status", "-sb"]).lines().count() == 1);
    }

    #[tokio::test]
    async fn fixup_goes_into_an_old_commit_and_squash_keeps_the_message() {
        let repo = Repo::new();
        commit(&repo, "a", "1\n", "one");
        let two = commit(&repo, "b", "1\n", "two");
        commit(&repo, "c", "1\n", "three");
        write(&repo, "b", "fixed\n");
        repo.git(&["add", "b"]);
        write(&repo, "c", "not staged\n");
        fixup(repo.path(), &two, false).await.unwrap();
        assert_eq!(subjects(&repo), ["three", "two", "one"]);
        assert_eq!(repo.git(&["show", "HEAD~:b"]), "fixed\n");
        assert_eq!(read(&repo, "c"), "not staged\n", "what is not staged stays");
        repo.git(&["checkout", "c"]);
        // Squash into: the commit stays, with its message.
        let two = repo.git(&["rev-parse", "HEAD~"]).trim().to_string();
        write(&repo, "b", "again\n");
        repo.git(&["add", "b"]);
        fixup(repo.path(), &two, true).await.unwrap();
        assert_eq!(subjects(&repo), ["three", "two", "one"]);
        assert_eq!(repo.git(&["show", "HEAD~:b"]), "again\n");
        // Nothing staged: nothing to do.
        assert!(fixup(repo.path(), &two, false).await.is_err());
    }

    /// `one`, then `both` changing `a` and adding `b`, then `last`.
    fn three_commits() -> (Repo, String) {
        let repo = Repo::new();
        commit(&repo, "a", "1\n", "one");
        write(&repo, "a", "2\n");
        let both = commit(&repo, "b", "new\n", "both");
        commit(&repo, "c", "c\n", "last");
        (repo, both)
    }

    #[tokio::test]
    async fn files_are_taken_out_of_a_commit() {
        let (repo, both) = three_commits();
        remove_from_commit(repo.path(), &both, &["b".to_string()])
            .await
            .unwrap();
        assert_eq!(subjects(&repo), ["last", "both", "one"]);
        assert!(!repo.path().join("b").exists());
        assert_eq!(read(&repo, "a"), "2\n");
    }

    #[tokio::test]
    async fn every_file_of_a_commit_can_not_be_taken_out() {
        let (repo, both) = three_commits();
        let all = ["a".to_string(), "b".to_string()];
        let error = remove_from_commit(repo.path(), &both, &all)
            .await
            .unwrap_err();
        assert_eq!(error, t!("git.err_all_files"));
        assert_eq!(subjects(&repo), ["last", "both", "one"]);
    }

    #[tokio::test]
    async fn files_are_extracted_to_a_commit_of_their_own() {
        let (repo, both) = three_commits();
        extract_from_commit(repo.path(), &both, &["b".to_string()], "just b")
            .await
            .unwrap();
        assert_eq!(subjects(&repo), ["last", "just b", "both", "one"]);
        assert_eq!(read(&repo, "b"), "new\n");
        let in_both = repo.git(&["show", "--format=", "--name-only", "HEAD~2"]);
        assert_eq!(in_both.trim(), "a");
    }

    #[tokio::test]
    async fn the_changes_of_some_files_of_a_commit_are_applied_uncommitted() {
        let repo = Repo::new();
        commit(&repo, "a", "1\n", "one");
        repo.git(&["checkout", "-q", "-b", "topic"]);
        write(&repo, "a", "2\n");
        let both = commit(&repo, "b", "new\n", "both");
        repo.git(&["checkout", "-q", "main"]);
        apply_from_commit(repo.path(), &both, &["b".to_string()])
            .await
            .unwrap();
        assert_eq!(read(&repo, "b"), "new\n");
        assert_eq!(read(&repo, "a"), "1\n", "the other file is left");
        assert_eq!(subjects(&repo), ["one"]);
    }

    #[tokio::test]
    async fn patches_of_local_changes_and_of_commits_go_both_ways() {
        let repo = Repo::new();
        commit(&repo, "a", "1\n", "one");
        write(&repo, "a", "2\n");
        let patch = local_patch(repo.path()).await.unwrap();
        repo.git(&["checkout", "a"]);
        assert!(!is_mail_patch(&patch));
        apply_patch(repo.path(), &patch).await.unwrap();
        assert_eq!(read(&repo, "a"), "2\n");
        repo.git(&["checkout", "a"]);
        // A commit keeps its message.
        let sha = commit(&repo, "a", "3\n", "three");
        let patch = commits_patch(repo.path(), std::slice::from_ref(&sha))
            .await
            .unwrap();
        repo.git(&["reset", "-q", "--hard", "HEAD~"]);
        assert!(is_mail_patch(&patch));
        apply_patch(repo.path(), &patch).await.unwrap();
        assert_eq!(subjects(&repo), ["three", "one"]);
        assert!(apply_patch(repo.path(), " ").await.is_err());
    }

    #[tokio::test]
    async fn a_merge_without_commit_and_with_a_message() {
        let repo = Repo::new();
        commit(&repo, "a", "1\n", "one");
        repo.git(&["checkout", "-q", "-b", "topic"]);
        commit(&repo, "b", "1\n", "topic work");
        repo.git(&["checkout", "-q", "main"]);
        let options = MergeOptions {
            mode: MergeMode::NoFastForward,
            no_commit: true,
            ..MergeOptions::default()
        };
        merge(repo.path(), "topic", &options).await.unwrap();
        assert_eq!(subjects(&repo), ["one"], "nothing committed yet");
        repo.git(&["merge", "--abort"]);
        let options = MergeOptions {
            mode: MergeMode::NoFastForward,
            message: "my merge".into(),
            ..MergeOptions::default()
        };
        merge(repo.path(), "topic", &options).await.unwrap();
        assert_eq!(subjects(&repo)[0], "my merge");
    }

    #[tokio::test]
    async fn a_rebase_onto_another_base() {
        let repo = Repo::new();
        commit(&repo, "a", "1\n", "base");
        repo.git(&["checkout", "-q", "-b", "next"]);
        commit(&repo, "n", "1\n", "next work");
        repo.git(&["checkout", "-q", "-b", "topic"]);
        commit(&repo, "t", "1\n", "topic work");
        // `topic` leaves `next` for `main`.
        let options = RebaseOptions {
            onto: "main".into(),
            ..RebaseOptions::default()
        };
        rebase(repo.path(), "next", &options).await.unwrap();
        assert_eq!(subjects(&repo), ["topic work", "base"]);
    }

    #[test]
    fn rebase_options_make_their_flags() {
        let options = RebaseOptions {
            rebase_merges: true,
            keep_empty: true,
            update_refs: true,
            autosquash: true,
            root: true,
            ..RebaseOptions::default()
        };
        assert_eq!(
            options.args("x"),
            [
                "rebase",
                "-i",
                "--autosquash",
                "--rebase-merges",
                "--keep-empty",
                "--update-refs",
                "--root"
            ]
        );
    }
}
