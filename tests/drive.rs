//! The plugin as the editor runs it: a process spoken to in JSON-RPC, on a
//! repository made for the test.

#![cfg(unix)]

use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use ardoise_plugin::protocol::{methods, read_message, write_message, Message};
use serde_json::{json, Value};

struct Editor {
    child: Child,
    input: ChildStdin,
    from: Receiver<Message>,
    /// What the editor answers to the plugin's requests, by method.
    answers: Vec<(String, Value)>,
    next: i64,
}

impl Editor {
    fn start(data: &Path, workspace: &Path) -> Editor {
        let mut child = Command::new(env!("CARGO_BIN_EXE_git"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the plugin starts");
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (to, from) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            while let Ok(Some(message)) = read_message(&mut reader) {
                if to.send(message).is_err() {
                    break;
                }
            }
        });
        let mut editor = Editor {
            child,
            input,
            from,
            answers: Vec::new(),
            next: 1,
        };
        editor.request(
            methods::INITIALIZE,
            json!({
                "api": 1,
                "app_version": "1.0.0",
                "settings": {"locale": "en", "dark": false},
                "workspace": [workspace],
                "plugin_dir": data,
                "data_dir": data,
            }),
        );
        editor
    }

    fn send(&mut self, message: &Message) {
        write_message(&mut self.input, message).unwrap();
        self.input.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) {
        let id = self.next;
        self.next += 1;
        self.send(&Message::request(id, method, params));
    }

    /// Asks the plugin, and waits for its answer.
    fn ask(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.request(method, params);
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.from.recv_timeout(left) {
                Ok(Message::Response { id: got, result }) if got == id => {
                    return result.expect("an answer");
                }
                Ok(Message::Request { id, .. }) => self.send(&Message::Response {
                    id,
                    result: Ok(Value::Null),
                }),
                Ok(_) => {}
                Err(_) => panic!("no answer to {method}"),
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&Message::notification(method, params));
    }

    fn event(&mut self, action: &str, value: Value) {
        let mut params = json!({"panel": "git", "action": action});
        if !value.is_null() {
            params["value"] = value;
        }
        self.notify(methods::UI_EVENT, params);
    }

    /// Waits for a message of the plugin (a notification or a request) of
    /// that method whose parameters pass `test`; answers its requests on
    /// the way.
    fn wait(&mut self, method: &str, test: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let message = self
                .from
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no {method} came"));
            let (name, params, id) = match message {
                Message::Notification { method, params } => (method, params, None),
                Message::Request { id, method, params } => (method, params, Some(id)),
                Message::Response { .. } => continue,
            };
            if let Some(id) = id {
                let answer = self
                    .answers
                    .iter()
                    .find(|(m, _)| *m == name)
                    .map_or(Value::Null, |(_, v)| v.clone());
                self.send(&Message::Response {
                    id,
                    result: Ok(answer),
                });
            }
            if name == method && test(&params) {
                return params;
            }
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn has(value: &Value, needle: &str) -> bool {
    value.to_string().contains(needle)
}

#[test]
fn the_plugin_shows_a_repository_and_commits_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = std::fs::canonicalize(dir.path()).unwrap().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let data = tempfile::tempdir().unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Ada"]);
    git(&repo, &["config", "user.email", "ada@example.com"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    std::fs::write(repo.join("a.txt"), "one\nTWO\n").unwrap();
    std::fs::write(repo.join("b.txt"), "new\n").unwrap();

    let mut editor = Editor::start(data.path(), &repo);

    // The files of the project are colored.
    let a = repo.join("a.txt").display().to_string();
    let b = repo.join("b.txt").display().to_string();
    editor.wait(methods::DECORATE, |p| {
        let files = p["files"].as_array().cloned().unwrap_or_default();
        files
            .iter()
            .any(|f| f["path"] == a.as_str() && f["state"] == "modified")
            && files
                .iter()
                .any(|f| f["path"] == b.as_str() && f["state"] == "untracked")
    });

    // The file in front is compared with what the last commit has.
    editor.notify(methods::ACTIVE_FILE_CHANGED, json!({"path": a}));
    editor.wait(methods::SET_BASE, |p| {
        p["path"] == a.as_str() && p["text"] == "one\ntwo\n"
    });

    // The panel lists the changes and the branch.
    editor.notify(methods::PANEL_OPENED, json!({"panel": "git"}));
    editor.wait(methods::RENDER, |p| {
        p["panel"] == "git" && has(p, "a.txt") && has(p, "b.txt") && has(p, "main")
    });

    // A file picked: its diff in a tab of the editor.
    editor.event("unstaged", json!("a.txt"));
    let tab = editor.wait(methods::OPEN_DIFF_TAB, |p| p["tab"] == "changes:unstaged");
    let file = &tab["files"][tab["start"].as_u64().unwrap() as usize];
    assert_eq!(file["path"], "a.txt");
    assert_eq!(file["mode"], "unstaged");
    assert!(has(&file["patch"], "-two") && has(&file["patch"], "+TWO"));

    // A block staged from the tab, then committed from the panel.
    let patch = file["patch"].as_str().unwrap().to_string();
    editor.notify(
        methods::DIFF_ACTION,
        json!({"tab": "changes:unstaged", "file": tab["start"], "action": {
            "kind": "apply_to_index", "patch": patch, "reverse": false
        }}),
    );
    editor.wait(methods::RENDER, |p| has(p, "Staged (1)"));
    editor.event("message", json!("Second"));
    editor.event("commit", Value::Null);
    editor.wait(methods::RENDER, |p| has(p, "Committed."));
    assert!(git(&repo, &["log", "--format=%s"]).starts_with("Second"));
    // What the file is compared with follows the commit.
    editor.wait(methods::SET_BASE, |p| p["text"] == "one\nTWO\n");

    // The log, with the commit picked.
    editor.event("tab:log", Value::Null);
    editor.wait(methods::RENDER, |p| has(p, "Second") && has(p, "first"));

    // A new branch, from the Branches tab.
    editor.event("tab:branches", Value::Null);
    editor.event("ref:create", Value::Null);
    editor.event("ref_name", json!("topic"));
    editor.event("ref_dialog:ok", Value::Null);
    editor.wait(methods::RENDER, |p| has(p, "b:topic"));
    assert_eq!(git(&repo, &["branch", "--show-current"]).trim(), "topic");

    // A stash of what is left (the untracked file stays).
    std::fs::write(repo.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    editor.event("tab:stash", Value::Null);
    editor.event("stash_message", json!("later"));
    editor.event("stash:push", Value::Null);
    editor.wait(methods::RENDER, |p| has(p, "later"));
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap(),
        "one\nTWO\n"
    );

    // The blame of the file in front, beside its lines.
    editor.answers.push((
        methods::ACTIVE_FILE.to_string(),
        json!({"path": a, "text": "one\nTWO\nmore\n", "lines": [1, 1]}),
    ));
    editor.notify(
        methods::COMMAND,
        json!({"command": "git.blame", "context": {"file": a}}),
    );
    let notes = editor.wait(methods::ANNOTATE, |p| p["path"] == a.as_str());
    let lines = notes["annotations"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3);
    assert!(has(&lines[0], "Ada"));
    assert_eq!(lines[2]["text"], "Not committed yet");
    assert!(
        has(&notes["annotations"]["details"], "Second")
            || has(&notes["annotations"]["details"], "first")
    );

    // Search Everywhere: the branches and the commits, and one picked.
    editor.notify(methods::SEARCH_OPENED, Value::Null);
    let items = editor.wait(methods::SEARCH_ITEMS, |p| has(p, "branch:topic"));
    assert!(has(&items, "Second") && has(&items, "branch:main"));
    let sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
    editor.notify(
        methods::SEARCH_PICKED,
        json!({"id": format!("commit:{sha}")}),
    );
    editor.wait(methods::SHOW_PANEL, |p| p["panel"] == "git");
    editor.wait(methods::RENDER, |p| has(p, "\"tab:log\"") && has(p, &sha));
    editor.notify(methods::SEARCH_PICKED, json!({"id": "branch:main"}));
    let end = Instant::now() + Duration::from_secs(20);
    while git(&repo, &["branch", "--show-current"]).trim() != "main" {
        assert!(Instant::now() < end, "main checked out");
        let _ = editor.from.recv_timeout(Duration::from_millis(200));
    }
    git(&repo, &["checkout", "-q", "topic"]);

    // A conflict, given to the editor to settle, then staged once settled.
    git(&repo, &["checkout", "-q", "-b", "other", "main~1"]);
    std::fs::write(repo.join("a.txt"), "one\nother\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "other side"]);
    git(&repo, &["checkout", "-q", "topic"]);
    git(&repo, &["merge", "-q", "other"]);
    editor.event("refresh", Value::Null);
    editor.event("tab:commit", Value::Null);
    editor.wait(methods::RENDER, |p| has(p, "Conflicts (1)"));
    editor.event("conflicts", json!("a.txt"));
    editor.event("merge_file", Value::Null);
    let merge = editor.wait(methods::OPEN_MERGE, |p| p["path"] == a.as_str());
    assert!(has(&merge["text"], "<<<<<<<") && has(&merge["text"], "other"));
    std::fs::write(repo.join("a.txt"), "one\nboth\n").unwrap();
    editor.notify(
        methods::MERGE_RESOLVED,
        json!({"tab": merge["tab"], "path": a, "text": "one\nboth\n"}),
    );
    editor.wait(methods::RENDER, |p| has(p, "Merge: conflicts settled"));
    assert!(!git(&repo, &["diff", "--name-only", "--diff-filter=U"]).contains("a.txt"));
}

#[test]
fn outside_of_a_repository_the_panel_offers_to_make_one() {
    let dir = tempfile::tempdir().unwrap();
    let project = std::fs::canonicalize(dir.path()).unwrap().join("project");
    std::fs::create_dir(&project).unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut editor = Editor::start(data.path(), &project);
    editor.notify(methods::PANEL_OPENED, json!({"panel": "git"}));
    editor.wait(methods::RENDER, |p| {
        has(p, "not a Git repository") && has(p, "\"init\"")
    });
    editor.event("init", Value::Null);
    editor.wait(methods::RENDER, |p| {
        has(p, "tab:commit") && has(p, "Nothing to commit")
    });
    assert!(project.join(".git").is_dir());
}

#[test]
fn deleting_a_tracked_file_offers_to_take_it_out_of_git_too() {
    let dir = tempfile::tempdir().unwrap();
    let repo = std::fs::canonicalize(dir.path()).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("kept.txt"), "a\n").unwrap();
    std::fs::write(repo.join("loose.txt"), "b\n").unwrap();
    git(&repo, &["add", "kept.txt"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    let data = tempfile::tempdir().unwrap();
    let mut editor = Editor::start(data.path(), &repo);
    let tracked = editor.ask(
        methods::DELETE_OPTIONS,
        json!({"path": repo.join("kept.txt")}),
    );
    assert_eq!(tracked["options"][0]["id"], "git_rm");
    assert_eq!(tracked["options"][0]["checked"], true);
    assert_eq!(tracked["options"][0]["label"], "Also remove it from Git");
    let loose = editor.ask(
        methods::DELETE_OPTIONS,
        json!({"path": repo.join("loose.txt")}),
    );
    assert_eq!(loose["options"], json!([]), "git does not track it");
    // The box left ticked: the deletion is staged.
    editor.notify(
        methods::FILE_DELETED,
        json!({"path": repo.join("kept.txt"), "options": ["git_rm"]}),
    );
    let end = Instant::now() + Duration::from_secs(20);
    while !git(&repo, &["status", "--porcelain"]).contains("D  kept.txt") {
        assert!(
            Instant::now() < end,
            "{}",
            git(&repo, &["status", "--porcelain"])
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn the_settings_are_kept() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut editor = Editor::start(data.path(), project.path());
    editor.notify(methods::PANEL_OPENED, json!({"panel": "settings"}));
    editor.wait(methods::RENDER, |p| p["panel"] == "settings");
    editor.notify(
        methods::UI_EVENT,
        json!({"panel": "settings", "action": "pull_mode", "value": "Pull --rebase"}),
    );
    editor.notify(
        methods::UI_EVENT,
        json!({"panel": "settings", "action": "auto_fetch", "value": "15"}),
    );
    editor.wait(methods::RENDER, |p| {
        p["panel"] == "settings" && has(p, "15")
    });
    let kept: Value =
        serde_json::from_str(&std::fs::read_to_string(data.path().join("settings.json")).unwrap())
            .unwrap();
    assert_eq!(kept["pull_mode"], "rebase");
    assert_eq!(kept["auto_fetch_minutes"], 15);
}
