// End-to-end integration tests for Pulse.
//
// Each test spins up the reference Bun server on a random port,
// creates a temp repo directory, and exercises the CLI binary
// against the real server over HTTP.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use tempfile::TempDir;

/// Find a free port by binding to port 0 and reading the assigned port.
fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to free port");
    listener.local_addr().unwrap().port()
}

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

struct TestEnv {
    repo_dir: TempDir,
    _db_dir: TempDir,
    server: Child,
    port: u16,
    pulse_bin: PathBuf,
}

impl TestEnv {
    /// Spin up a fresh server + repo directory.
    fn new() -> Self {
        let repo_dir = TempDir::new().expect("create repo tempdir");
        let db_dir = TempDir::new().expect("create db tempdir");
        let port = find_free_port();

        let db_path = db_dir.path().join("test.db");
        let server_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/server");

        let server = Command::new("bun")
            .arg("run")
            .arg("src/index.ts")
            .current_dir(&server_dir)
            .env("PORT", port.to_string())
            .env("PULSE_DB", db_path.to_str().unwrap())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start bun server");

        // Wait for the server to become available by polling the port.
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > std::time::Duration::from_secs(10) {
                panic!("server did not start within 10 seconds");
            }
            match std::net::TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(_) => break,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
        // Small extra delay to ensure Bun's HTTP handler is fully ready
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Resolve the binary built by cargo.
        let pulse_bin = PathBuf::from(env!("CARGO_BIN_EXE_pulse"));

        Self {
            repo_dir,
            _db_dir: db_dir,
            server,
            port,
            pulse_bin,
        }
    }

    fn remote_url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Run `pulse init --remote <url>` inside the repo dir.
    fn init_repo(&self) -> CmdResult {
        self.pulse(&["init", "--remote", &self.remote_url()])
    }

    /// Run `pulse <args>` inside the repo dir.
    fn pulse(&self, args: &[&str]) -> CmdResult {
        let output = Command::new(&self.pulse_bin)
            .args(args)
            .current_dir(self.repo_dir.path())
            .env("USER", "testuser")
            .output()
            .expect("run pulse command");

        CmdResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            success: output.status.success(),
            code: output.status.code(),
        }
    }

    /// Run pulse with a custom USER env var.
    fn pulse_as(&self, user: &str, args: &[&str]) -> CmdResult {
        let output = Command::new(&self.pulse_bin)
            .args(args)
            .current_dir(self.repo_dir.path())
            .env("USER", user)
            .output()
            .expect("run pulse command");

        CmdResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            success: output.status.success(),
            code: output.status.code(),
        }
    }

    /// Run pulse from a different repo dir (for clone tests), sharing this server.
    fn pulse_in(&self, dir: &Path, args: &[&str]) -> CmdResult {
        let output = Command::new(&self.pulse_bin)
            .args(args)
            .current_dir(dir)
            .env("USER", "testuser")
            .output()
            .expect("run pulse command");

        CmdResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            success: output.status.success(),
            code: output.status.code(),
        }
    }

    /// Write a file into the repo directory, creating parent dirs as needed.
    fn write_file(&self, rel_path: &str, content: &[u8]) {
        let path = self.repo_dir.path().join(rel_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(&path, content).expect("write file");
    }

    /// Read a file from the repo directory. Returns None if it doesn't exist.
    fn read_file(&self, rel_path: &str) -> Option<Vec<u8>> {
        let path = self.repo_dir.path().join(rel_path);
        std::fs::read(&path).ok()
    }

    /// Check if a file exists in the repo directory.
    fn file_exists(&self, rel_path: &str) -> bool {
        self.repo_dir.path().join(rel_path).exists()
    }

    /// Extract a workspace id from `pulse workspace create` output.
    /// Looks for "Created workspace ws-XXXX" pattern.
    fn extract_workspace_id(output: &str) -> String {
        for line in output.lines() {
            if line.starts_with("Created workspace ") {
                return line
                    .strip_prefix("Created workspace ")
                    .unwrap()
                    .trim()
                    .to_string();
            }
        }
        panic!("could not find workspace id in output:\n{output}");
    }

    /// Helper: init repo, create workspace, return workspace id.
    fn setup_workspace(&self, intent: &str, scope: &[&str]) -> String {
        let r = self.init_repo();
        assert!(r.success, "init failed: {}\n{}", r.stdout, r.stderr);

        let mut args = vec!["workspace", "create", "--intent", intent];
        for s in scope {
            args.push("--scope");
            args.push(s);
        }
        let r = self.pulse(&args);
        assert!(r.success, "workspace create failed: {}\n{}", r.stdout, r.stderr);
        Self::extract_workspace_id(&r.stdout)
    }

    /// Helper: full roundtrip -- init, create workspace, write file, commit, merge, show.
    /// Returns the content read back via `pulse show`.
    fn roundtrip_file(&self, path: &str, content: &[u8]) -> Vec<u8> {
        let ws_id = self.setup_workspace("roundtrip test", &[]);
        self.write_file(path, content);

        let r = self.pulse(&["commit", "-m", "add file", "-w", &ws_id, path]);
        assert!(r.success, "commit failed: {}\n{}", r.stdout, r.stderr);

        let r = self.pulse(&["merge", &ws_id]);
        assert!(r.success, "merge failed: {}\n{}", r.stdout, r.stderr);

        // `pulse show` writes raw bytes to stdout
        let output = Command::new(&self.pulse_bin)
            .args(["show", path])
            .current_dir(self.repo_dir.path())
            .env("USER", "testuser")
            .output()
            .expect("run pulse show");
        assert!(output.status.success(), "show failed: {}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

#[derive(Debug)]
struct CmdResult {
    stdout: String,
    stderr: String,
    success: bool,
    #[allow(dead_code)]
    code: Option<i32>,
}

// ===========================================================================
// Group 1: Basic Lifecycle
// ===========================================================================

#[test]
fn init_creates_pulse_dir() {
    let env = TestEnv::new();
    let r = env.init_repo();
    assert!(r.success, "init failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Initialized Pulse repository"));
    assert!(env.repo_dir.path().join(".pulse").exists());
}

#[test]
fn status_shows_repo_info() {
    let env = TestEnv::new();
    env.init_repo();
    let r = env.pulse(&["status"]);
    assert!(r.success, "status failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("main:"));
    assert!(r.stdout.contains("workspaces:  0 active"));
    assert!(r.stdout.contains(&env.remote_url()));
}

#[test]
fn init_already_initialized() {
    let env = TestEnv::new();
    env.init_repo();
    let r = env.init_repo();
    assert!(!r.success, "second init should fail");
}

// ===========================================================================
// Group 2: Workspace Lifecycle
// ===========================================================================

#[test]
fn workspace_create_and_list() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("add logging", &["src/**"]);
    assert!(ws_id.starts_with("ws-"));

    let r = env.pulse(&["workspace", "list"]);
    assert!(r.success);
    assert!(r.stdout.contains(&ws_id));
    assert!(r.stdout.contains("add logging"));
}

#[test]
fn workspace_status_shows_details() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("implement auth", &["src/auth/**"]);

    let r = env.pulse(&["workspace", "status", &ws_id]);
    assert!(r.success);
    assert!(r.stdout.contains("implement auth"));
    assert!(r.stdout.contains("active"));
    assert!(r.stdout.contains("testuser"));
}

#[test]
fn workspace_abandon() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("will be abandoned", &[]);

    let r = env.pulse(&["workspace", "abandon", &ws_id]);
    assert!(r.success);
    assert!(r.stdout.contains("Abandoned"));

    // Should not appear in active list
    let r = env.pulse(&["workspace", "list"]);
    assert!(r.success);
    assert!(!r.stdout.contains(&ws_id));

    // Should appear in --all list
    let r = env.pulse(&["workspace", "list", "--all"]);
    assert!(r.success);
    assert!(r.stdout.contains(&ws_id));
    assert!(r.stdout.contains("abandoned"));
}

// ===========================================================================
// Group 3: Commit & Content Roundtrip
// ===========================================================================

#[test]
fn commit_single_file() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("single file commit", &[]);

    env.write_file("hello.txt", b"hello world");
    let r = env.pulse(&["commit", "-m", "add hello", "-w", &ws_id, "hello.txt"]);
    assert!(r.success, "commit failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Committed"));
    assert!(r.stdout.contains("hello.txt"));
}

#[test]
fn commit_multiple_files() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("multi file commit", &[]);

    env.write_file("a.txt", b"file a");
    env.write_file("b.txt", b"file b");
    env.write_file("c.txt", b"file c");

    let r = env.pulse(&[
        "commit", "-m", "add files", "-w", &ws_id,
        "a.txt", "b.txt", "c.txt",
    ]);
    assert!(r.success, "commit failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("a.txt"));
    assert!(r.stdout.contains("b.txt"));
    assert!(r.stdout.contains("c.txt"));
}

#[test]
fn commit_then_show() {
    let env = TestEnv::new();
    let content = b"fn main() { println!(\"hello from pulse\"); }";
    let readback = env.roundtrip_file("src/main.rs", content);
    assert_eq!(readback, content);
}

#[test]
fn commit_then_files() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("files listing", &[]);

    env.write_file("src/lib.rs", b"pub fn add(a: i32, b: i32) -> i32 { a + b }");
    env.write_file("README.md", b"# My Project");

    let r = env.pulse(&[
        "commit", "-m", "add files", "-w", &ws_id,
        "src/lib.rs", "README.md",
    ]);
    assert!(r.success);

    let r = env.pulse(&["merge", &ws_id]);
    assert!(r.success);

    let r = env.pulse(&["files"]);
    assert!(r.success);
    assert!(r.stdout.contains("src/lib.rs"));
    assert!(r.stdout.contains("README.md"));
}

// ===========================================================================
// Group 4: Multi-Language File Roundtrips
// ===========================================================================

#[test]
fn roundtrip_rust() {
    let env = TestEnv::new();
    let content = b"use std::collections::HashMap;\n\n\
/// A generic cache with TTL support.\n\
/// Supports unicode: kakkoii (\xe3\x81\x8b\xe3\x81\xa3\xe3\x81\x93\xe3\x81\x84\xe3\x81\x84)\n\
pub struct Cache<K: std::hash::Hash + Eq, V: Clone> {\n\
    inner: HashMap<K, (V, std::time::Instant)>,\n\
    ttl: std::time::Duration,\n\
}\n\n\
impl<K: std::hash::Hash + Eq, V: Clone> Cache<K, V> {\n\
    pub fn new(ttl: std::time::Duration) -> Self {\n\
        Self { inner: HashMap::new(), ttl }\n\
    }\n\n\
    pub fn get(&self, key: &K) -> Option<&V> {\n\
        self.inner.get(key).and_then(|(v, ts)| {\n\
            if ts.elapsed() < self.ttl { Some(v) } else { None }\n\
        })\n\
    }\n\
}\n";
    let readback = env.roundtrip_file("src/cache.rs", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_typescript() {
    let env = TestEnv::new();
    let content = b"import { Hono } from \"hono\";\n\n\
interface User<T extends Record<string, unknown> = {}> {\n\
  id: string;\n\
  email: string;\n\
  metadata: T;\n\
  createdAt: Date;\n\
}\n\n\
type CreateUser<T extends Record<string, unknown>> = Omit<User<T>, \"id\" | \"createdAt\">;\n\n\
async function fetchUser<T extends Record<string, unknown>>(\n\
  id: string,\n\
): Promise<User<T> | null> {\n\
  const resp = await fetch(`/api/users/${id}`);\n\
  if (!resp.ok) return null;\n\
  return resp.json() as Promise<User<T>>;\n\
}\n\n\
const app = new Hono();\n\n\
app.get(\"/users/:id\", async (c) => {\n\
  const user = await fetchUser(c.req.param(\"id\"));\n\
  if (!user) return c.json({ error: \"not found\" }, 404);\n\
  return c.json(user);\n\
});\n\n\
export { app, fetchUser, type User, type CreateUser };\n";
    let readback = env.roundtrip_file("src/index.ts", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_python() {
    let env = TestEnv::new();
    let content = b"\"\"\"\n\
Module docstring with unicode: naive Bayes classifier\n\
Supports: espanol, francais, Deutsch, nihongo (\xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e)\n\
\"\"\"\n\
from __future__ import annotations\n\n\
import asyncio\n\
from dataclasses import dataclass, field\n\
from typing import Generic, TypeVar\n\n\
T = TypeVar(\"T\")\n\n\n\
@dataclass\n\
class PriorityQueue(Generic[T]):\n\
    \"\"\"A simple priority queue using a sorted list.\"\"\"\n\
    _items: list[tuple[float, T]] = field(default_factory=list)\n\n\
    def push(self, priority: float, item: T) -> None:\n\
        self._items.append((priority, item))\n\
        self._items.sort(key=lambda x: x[0])\n\n\
    def pop(self) -> T:\n\
        if not self._items:\n\
            raise IndexError(\"pop from empty queue\")\n\
        return self._items.pop(0)[1]\n\n\
    @property\n\
    def is_empty(self) -> bool:\n\
        return len(self._items) == 0\n\n\n\
async def process_batch(items: list[str]) -> list[str]:\n\
    results = []\n\
    for i, item in enumerate(items):\n\
        await asyncio.sleep(0.001)\n\
        results.append(f\"Processed #{i}: {item!r}\")\n\
    return results\n";
    let readback = env.roundtrip_file("main.py", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_go() {
    let env = TestEnv::new();
    let content = b"package main\n\n\
import (\n\
\t\"context\"\n\
\t\"fmt\"\n\
\t\"sync\"\n\
\t\"time\"\n\
)\n\n\
type Worker struct {\n\
\tID       int    `json:\"id\"`\n\
\tName     string `json:\"name,omitempty\"`\n\
\tmu       sync.Mutex\n\
\tjobCount int\n\
}\n\n\
func (w *Worker) Process(ctx context.Context, jobs <-chan string) error {\n\
\tfor {\n\
\t\tselect {\n\
\t\tcase <-ctx.Done():\n\
\t\t\treturn ctx.Err()\n\
\t\tcase job, ok := <-jobs:\n\
\t\t\tif !ok {\n\
\t\t\t\treturn nil\n\
\t\t\t}\n\
\t\t\tw.mu.Lock()\n\
\t\t\tw.jobCount++\n\
\t\t\tw.mu.Unlock()\n\
\t\t\tfmt.Printf(\"Worker %d processed %q\\n\", w.ID, job)\n\
\t\t}\n\
\t}\n\
}\n\n\
func main() {\n\
\tctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)\n\
\tdefer cancel()\n\
\t_ = ctx\n\
}\n";
    let readback = env.roundtrip_file("main.go", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_swift() {
    let env = TestEnv::new();
    let content = b"import SwiftUI\n\
import Observation\n\n\
@Observable\n\
final class AppState {\n\
    var count = 0\n\
    var items: [Item] = []\n\n\
    struct Item: Identifiable, Sendable {\n\
        let id: UUID\n\
        var title: String\n\
        var isCompleted: Bool\n\
    }\n\n\
    func toggle(_ item: Item) {\n\
        guard let index = items.firstIndex(where: { $0.id == item.id }) else { return }\n\
        items[index].isCompleted.toggle()\n\
    }\n\
}\n\n\
struct ContentView: View {\n\
    @State private var state = AppState()\n\n\
    var body: some View {\n\
        NavigationStack {\n\
            List(state.items) { item in\n\
                HStack {\n\
                    Text(item.title)\n\
                    Spacer()\n\
                    Image(systemName: item.isCompleted ? \"checkmark.circle.fill\" : \"circle\")\n\
                }\n\
                .onTapGesture { state.toggle(item) }\n\
            }\n\
            .navigationTitle(\"Tasks\")\n\
        }\n\
    }\n\
}\n";
    let readback = env.roundtrip_file("App.swift", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_java() {
    let env = TestEnv::new();
    let content = b"package com.example;\n\n\
import java.util.*;\n\
import java.util.concurrent.CompletableFuture;\n\n\
public sealed interface Expr permits Expr.Num, Expr.Add, Expr.Mul {\n\
    record Num(double value) implements Expr {}\n\
    record Add(Expr left, Expr right) implements Expr {}\n\
    record Mul(Expr left, Expr right) implements Expr {}\n\n\
    default double compute() {\n\
        return switch (this) {\n\
            case Num(var v) -> v;\n\
            case Add(var l, var r) -> l.compute() + r.compute();\n\
            case Mul(var l, var r) -> l.compute() * r.compute();\n\
        };\n\
    }\n\
}\n\n\
@FunctionalInterface\n\
interface Transformer<T> {\n\
    T transform(T input);\n\
}\n\n\
class Pipeline<T> {\n\
    private final List<Transformer<T>> stages = new ArrayList<>();\n\n\
    public Pipeline<T> addStage(Transformer<T> stage) {\n\
        stages.add(stage);\n\
        return this;\n\
    }\n\n\
    public CompletableFuture<T> executeAsync(T input) {\n\
        return CompletableFuture.supplyAsync(() -> {\n\
            T result = input;\n\
            for (var stage : stages) {\n\
                result = stage.transform(result);\n\
            }\n\
            return result;\n\
        });\n\
    }\n\
}\n";
    let readback = env.roundtrip_file("Main.java", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_cpp() {
    let env = TestEnv::new();
    let content = b"#include <concepts>\n\
#include <vector>\n\
#include <algorithm>\n\
#include <iostream>\n\
#include <ranges>\n\n\
template<typename T>\n\
concept Numeric = std::integral<T> || std::floating_point<T>;\n\n\
template<Numeric T>\n\
class StatAccumulator {\n\
    std::vector<T> data_;\n\n\
public:\n\
    void add(T value) { data_.push_back(value); }\n\n\
    T sum() const {\n\
        return std::accumulate(data_.begin(), data_.end(), T{});\n\
    }\n\n\
    double mean() const {\n\
        if (data_.empty()) return 0.0;\n\
        return static_cast<double>(sum()) / data_.size();\n\
    }\n\n\
    auto top_n(size_t n) const {\n\
        auto sorted = data_;\n\
        std::ranges::sort(sorted, std::greater{});\n\
        return sorted | std::views::take(n);\n\
    }\n\
};\n\n\
int main() {\n\
    StatAccumulator<double> acc;\n\
    for (auto v : {3.14, 2.71, 1.41}) {\n\
        acc.add(v);\n\
    }\n\
    std::cout << \"Mean: \" << acc.mean() << '\\n';\n\
    return 0;\n\
}\n";
    let readback = env.roundtrip_file("main.cpp", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_ruby() {
    let env = TestEnv::new();
    let content = b"# frozen_string_literal: true\n\n\
module Enumerable\n\
  def frequencies\n\
    each_with_object(Hash.new(0)) { |item, counts| counts[item] += 1 }\n\
  end\n\
end\n\n\
class EventBus\n\
  def initialize\n\
    @listeners = Hash.new { |h, k| h[k] = [] }\n\
  end\n\n\
  def on(event, &block)\n\
    @listeners[event] << block\n\
    self\n\
  end\n\n\
  def emit(event, **payload)\n\
    @listeners[event].each { |handler| handler.call(**payload) }\n\
  end\n\
end\n\n\
class User < Model\n\
  field :name, type: String\n\
  field :age, type: Integer, default: 0\n\
end\n\n\
user = User.new\n\
user.name = \"Alice\"\n\
puts \"#{user.name} (#{user.age})\"\n";
    let readback = env.roundtrip_file("app.rb", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_json() {
    let env = TestEnv::new();
    let content = b"{\n\
  \"name\": \"pulse\",\n\
  \"version\": \"0.1.0\",\n\
  \"config\": {\n\
    \"database\": {\n\
      \"host\": \"localhost\",\n\
      \"port\": 5432\n\
    },\n\
    \"features\": [true, false, null, 42, 3.14, -1],\n\
    \"tags\": [\"alpha\", \"beta\", \"gamma\"],\n\
    \"nested\": {\n\
      \"deeply\": {\n\
        \"nested\": {\n\
          \"value\": \"found it!\"\n\
        }\n\
      }\n\
    }\n\
  },\n\
  \"empty_object\": {},\n\
  \"empty_array\": []\n\
}\n";
    let readback = env.roundtrip_file("config.json", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_yaml() {
    let env = TestEnv::new();
    let content = b"apiVersion: apps/v1\n\
kind: Deployment\n\
metadata:\n\
  name: pulse-server\n\
  labels:\n\
    app: pulse\n\
    tier: backend\n\n\
spec:\n\
  replicas: 3\n\
  selector:\n\
    matchLabels:\n\
      app: pulse\n\
  template:\n\
    spec:\n\
      containers:\n\
        - name: server\n\
          image: pulse:latest\n\
          ports:\n\
            - containerPort: 3000\n\n\
  # Multiline string examples\n\
  description: |\n\
    This is a multiline description\n\
    that preserves newlines.\n\n\
# Anchors and aliases\n\
defaults: &defaults\n\
  timeout: 30\n\
  retries: 3\n\n\
production:\n\
  <<: *defaults\n\
  timeout: 60\n";
    let readback = env.roundtrip_file("deploy.yaml", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_toml() {
    let env = TestEnv::new();
    let content = b"[package]\n\
name = \"my-project\"\n\
version = \"0.1.0\"\n\
edition = \"2024\"\n\
authors = [\"Alice <alice@example.com>\", \"Bob <bob@example.com>\"]\n\n\
[dependencies]\n\
serde = { version = \"1\", features = [\"derive\"] }\n\
tokio = { version = \"1\", features = [\"full\"] }\n\
anyhow = \"1\"\n\n\
[dev-dependencies]\n\
tempfile = \"3\"\n\n\
[[bench]]\n\
name = \"storage_bench\"\n\
harness = false\n\n\
[profile.release]\n\
opt-level = 3\n\
lto = true\n";
    let readback = env.roundtrip_file("Cargo.toml", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_markdown() {
    let env = TestEnv::new();
    let content = b"# Pulse\n\n\
AI-native version control system.\n\n\
## Features\n\n\
| Feature | Status |\n\
|---------|--------|\n\
| Init | Done |\n\
| Workspaces | Done |\n\
| Merge | Done |\n\n\
## Quick Start\n\n\
```bash\n\
pulse init --remote https://api.pulse.dev\n\
pulse workspace create --intent \"add feature\"\n\
pulse commit -m \"implement feature\" -w ws-abcd src/main.rs\n\
pulse merge ws-abcd\n\
```\n\n\
> **Note**: This is a blockquote with **bold** and *italic* text.\n\n\
- [ ] Task 1\n\
- [x] Task 2\n\n\
---\n\n\
Made with care by the Pulse team.\n";
    let readback = env.roundtrip_file("README.md", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn roundtrip_binary() {
    let env = TestEnv::new();
    // PNG file header + IHDR chunk (minimal binary data with nulls and high bytes)
    let content: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, // IHDR length
        0x49, 0x48, 0x44, 0x52, // IHDR type
        0x00, 0x00, 0x00, 0x01, // width: 1
        0x00, 0x00, 0x00, 0x01, // height: 1
        0x08, 0x02,             // bit depth: 8, color type: RGB
        0x00, 0x00, 0x00,       // compression, filter, interlace
        0xFF, 0xFE, 0xFD, 0x00, // binary bytes with nulls
        0x01, 0x02, 0x03, 0x80, // more bytes including high bit
    ];
    let readback = env.roundtrip_file("icon.png", content);
    assert_eq!(readback, content);
}

#[test]
fn roundtrip_empty_file() {
    let env = TestEnv::new();
    let content: &[u8] = b"";
    let readback = env.roundtrip_file("empty.txt", content);
    assert_eq!(readback, content);
}

#[test]
fn roundtrip_large_file() {
    let env = TestEnv::new();
    // ~500KB file to exercise multi-chunk storage
    let mut content = Vec::with_capacity(500_000);
    for i in 0..10_000 {
        content.extend_from_slice(
            format!("// Line {i}: The quick brown fox jumps over the lazy dog. {i}\n").as_bytes(),
        );
    }
    assert!(content.len() > 400_000, "file should be >400KB");
    let readback = env.roundtrip_file("large_file.txt", &content);
    assert_eq!(readback, content);
}

// ===========================================================================
// Group 5: Merge Scenarios
// ===========================================================================

#[test]
fn merge_fast_forward() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("fast forward feature", &[]);

    env.write_file("feature.rs", b"pub fn feature() {}");
    let r = env.pulse(&["commit", "-m", "add feature", "-w", &ws_id, "feature.rs"]);
    assert!(r.success);

    let r = env.pulse(&["merge", &ws_id]);
    assert!(r.success);
    assert!(r.stdout.contains("Merged into main"));
    assert!(r.stdout.contains("feature.rs"));

    // Verify it appears in log
    let r = env.pulse(&["log"]);
    assert!(r.success);
    assert!(r.stdout.contains("Merge workspace"));
}

#[test]
fn merge_conflict_detected() {
    let env = TestEnv::new();
    env.init_repo();

    // Create two workspaces
    let r = env.pulse(&["workspace", "create", "--intent", "workspace A"]);
    assert!(r.success);
    let ws_a = TestEnv::extract_workspace_id(&r.stdout);

    let r = env.pulse(&["workspace", "create", "--intent", "workspace B"]);
    assert!(r.success);
    let ws_b = TestEnv::extract_workspace_id(&r.stdout);

    // Both modify the same file
    env.write_file("shared.rs", b"// version A");
    let r = env.pulse(&["commit", "-m", "modify shared (A)", "-w", &ws_a, "shared.rs"]);
    assert!(r.success);

    env.write_file("shared.rs", b"// version B");
    let r = env.pulse(&["commit", "-m", "modify shared (B)", "-w", &ws_b, "shared.rs"]);
    assert!(r.success);

    // Merge A first -- should succeed
    let r = env.pulse(&["merge", &ws_a]);
    assert!(r.success);
    assert!(r.stdout.contains("Merged into main"));

    // Merge B -- should detect conflict
    let r = env.pulse(&["merge", &ws_b]);
    assert!(r.success); // The command itself succeeds, it just reports a conflict
    assert!(r.stdout.contains("conflict"));
    assert!(r.stdout.contains("shared.rs"));
}

#[test]
fn merge_clean_three_way() {
    let env = TestEnv::new();
    env.init_repo();

    // Create two workspaces
    let r = env.pulse(&["workspace", "create", "--intent", "feature X"]);
    assert!(r.success);
    let ws_x = TestEnv::extract_workspace_id(&r.stdout);

    let r = env.pulse(&["workspace", "create", "--intent", "feature Y"]);
    assert!(r.success);
    let ws_y = TestEnv::extract_workspace_id(&r.stdout);

    // Each modifies different files
    env.write_file("x.rs", b"// feature X");
    let r = env.pulse(&["commit", "-m", "add X", "-w", &ws_x, "x.rs"]);
    assert!(r.success);

    env.write_file("y.rs", b"// feature Y");
    let r = env.pulse(&["commit", "-m", "add Y", "-w", &ws_y, "y.rs"]);
    assert!(r.success);

    // Merge X first
    let r = env.pulse(&["merge", &ws_x]);
    assert!(r.success);
    assert!(r.stdout.contains("Merged into main"));

    // Merge Y -- should succeed with three-way merge (no conflicts, different files)
    let r = env.pulse(&["merge", &ws_y]);
    assert!(r.success);
    assert!(r.stdout.contains("Merged into main"));

    // Trunk should have both files
    let r = env.pulse(&["files"]);
    assert!(r.success);
    assert!(r.stdout.contains("x.rs"));
    assert!(r.stdout.contains("y.rs"));
}

// ===========================================================================
// Group 6: Trunk History
// ===========================================================================

#[test]
fn log_shows_history() {
    let env = TestEnv::new();
    env.init_repo();

    // Create and merge 3 workspaces
    for i in 1..=3 {
        let r = env.pulse(&["workspace", "create", "--intent", &format!("feature {i}")]);
        assert!(r.success);
        let ws_id = TestEnv::extract_workspace_id(&r.stdout);

        env.write_file(&format!("file{i}.rs"), format!("// feature {i}").as_bytes());
        let r = env.pulse(&[
            "commit", "-m", &format!("add file{i}"), "-w", &ws_id,
            &format!("file{i}.rs"),
        ]);
        assert!(r.success);

        let r = env.pulse(&["merge", &ws_id]);
        assert!(r.success);
    }

    let r = env.pulse(&["log"]);
    assert!(r.success);

    // Should have 4 entries: root + 3 merges, most recent first
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert!(lines.len() >= 4, "expected at least 4 log lines, got {}", lines.len());

    // Most recent merge should be first
    assert!(lines[0].contains("Merge workspace"));
}

#[test]
fn log_author_filter() {
    let env = TestEnv::new();
    env.init_repo();

    // Commit as "alice"
    let r = env.pulse_as("alice", &["workspace", "create", "--intent", "alice work"]);
    assert!(r.success);
    let ws_alice = TestEnv::extract_workspace_id(&r.stdout);
    env.write_file("alice.rs", b"// alice");
    let r = env.pulse_as("alice", &["commit", "-m", "alice commit", "-w", &ws_alice, "alice.rs"]);
    assert!(r.success);
    let r = env.pulse_as("alice", &["merge", &ws_alice]);
    assert!(r.success);

    // Commit as "bob"
    let r = env.pulse_as("bob", &["workspace", "create", "--intent", "bob work"]);
    assert!(r.success);
    let ws_bob = TestEnv::extract_workspace_id(&r.stdout);
    env.write_file("bob.rs", b"// bob");
    let r = env.pulse_as("bob", &["commit", "-m", "bob commit", "-w", &ws_bob, "bob.rs"]);
    assert!(r.success);
    let r = env.pulse_as("bob", &["merge", &ws_bob]);
    assert!(r.success);

    // Filter by pulse (system author for merge changesets)
    let r = env.pulse(&["log", "--author", "pulse"]);
    assert!(r.success);
    // All merge changesets have author "pulse" (system), so this should list them
    let lines: Vec<&str> = r.stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(lines.len() >= 3, "expected merge changesets from pulse author");
}

#[test]
fn log_limit() {
    let env = TestEnv::new();
    env.init_repo();

    // Create a few merges
    for i in 1..=3 {
        let r = env.pulse(&["workspace", "create", "--intent", &format!("f{i}")]);
        assert!(r.success);
        let ws = TestEnv::extract_workspace_id(&r.stdout);
        env.write_file(&format!("f{i}.rs"), b"x");
        let r = env.pulse(&["commit", "-m", "c", "-w", &ws, &format!("f{i}.rs")]);
        assert!(r.success);
        let r = env.pulse(&["merge", &ws]);
        assert!(r.success);
    }

    let r = env.pulse(&["log", "--limit", "2"]);
    assert!(r.success);
    let lines: Vec<&str> = r.stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected exactly 2 log entries, got {}", lines.len());
}

// ===========================================================================
// Group 7: Diff
// ===========================================================================

// NOTE: `pulse diff` requires full 64-char hashes, but the CLI only outputs
// short (8-char) hashes. Diff is thoroughly tested at the unit level in
// src/core/diff.rs. To test this via CLI we would need the CLI to expose
// full hashes (a future improvement).

// ===========================================================================
// Group 8: Sync & Clone
// ===========================================================================

#[test]
fn push_then_clone() {
    let env = TestEnv::new();

    // Repo A: init, commit, merge
    let ws_id = env.setup_workspace("push content", &[]);
    env.write_file("hello.rs", b"fn hello() { println!(\"world\"); }");
    let r = env.pulse(&["commit", "-m", "add hello", "-w", &ws_id, "hello.rs"]);
    assert!(r.success);
    let r = env.pulse(&["merge", &ws_id]);
    assert!(r.success);

    // Repo B: init with same remote (clone behavior)
    let repo_b = TempDir::new().expect("create repo B tempdir");
    let r = env.pulse_in(
        repo_b.path(),
        &["init", "--remote", &env.remote_url()],
    );
    assert!(r.success, "clone failed: {}\n{}", r.stdout, r.stderr);

    // Repo B should have the file (auto-pull syncs on each command)
    let r = env.pulse_in(repo_b.path(), &["files"]);
    assert!(r.success, "files failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("hello.rs"), "expected hello.rs in files output:\n{}", r.stdout);

    // And the content should match
    let output = Command::new(&env.pulse_bin)
        .args(["show", "hello.rs"])
        .current_dir(repo_b.path())
        .env("USER", "testuser")
        .output()
        .expect("run pulse show");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"fn hello() { println!(\"world\"); }");
}

#[test]
fn clone_preserves_workspaces() {
    let env = TestEnv::new();
    env.init_repo();

    // Create a workspace from repo A
    let r = env.pulse(&["workspace", "create", "--intent", "from repo A"]);
    assert!(r.success);
    let ws_a = TestEnv::extract_workspace_id(&r.stdout);

    // Clone into repo B — should get the workspace from A
    let repo_b = TempDir::new().expect("create repo B tempdir");
    let r = env.pulse_in(repo_b.path(), &["init", "--remote", &env.remote_url()]);
    assert!(r.success);

    // Repo B should see repo A's workspace
    let r = env.pulse_in(repo_b.path(), &["workspace", "list"]);
    assert!(r.success);
    assert!(
        r.stdout.contains(&ws_a),
        "cloned repo should see workspace from source:\n{}",
        r.stdout
    );

    // Repo B can create its own workspace
    let r = env.pulse_in(repo_b.path(), &["workspace", "create", "--intent", "from repo B"]);
    assert!(r.success);
    let ws_b = TestEnv::extract_workspace_id(&r.stdout);

    // Repo B should see both workspaces
    let r = env.pulse_in(repo_b.path(), &["workspace", "list"]);
    assert!(r.success);
    assert!(r.stdout.contains(&ws_a));
    assert!(r.stdout.contains(&ws_b));
}

// ===========================================================================
// Group 9: Edge Cases
// ===========================================================================

#[test]
fn deeply_nested_paths() {
    let env = TestEnv::new();
    let content = b"deeply nested content";
    let readback = env.roundtrip_file("a/b/c/d/e/f/g/deep.rs", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn special_characters_in_paths() {
    let env = TestEnv::new();
    let content = b"file with special path";
    let readback = env.roundtrip_file("my-file.test.ts", content);
    assert_eq!(readback, content.as_slice());
}

#[test]
fn overwrite_file_in_second_commit() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("overwrite test", &[]);

    // First commit: v1
    env.write_file("config.rs", b"// version 1");
    let r = env.pulse(&["commit", "-m", "add config v1", "-w", &ws_id, "config.rs"]);
    assert!(r.success);

    // Second commit: v2 (overwrite)
    env.write_file("config.rs", b"// version 2 with more content");
    let r = env.pulse(&["commit", "-m", "update config v2", "-w", &ws_id, "config.rs"]);
    assert!(r.success);

    // Merge and verify v2 is what main has
    let r = env.pulse(&["merge", &ws_id]);
    assert!(r.success);

    let output = Command::new(&env.pulse_bin)
        .args(["show", "config.rs"])
        .current_dir(env.repo_dir.path())
        .env("USER", "testuser")
        .output()
        .expect("run pulse show");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"// version 2 with more content");
}

// ===========================================================================
// Group 10: Switch
// ===========================================================================

#[test]
fn switch_to_workspace_materializes_files() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("switch test", &[]);

    // Commit files to workspace
    env.write_file("src/main.rs", b"fn main() {}");
    env.write_file("README.md", b"# Hello");
    let r = env.pulse(&[
        "commit", "-m", "add files", "-w", &ws_id,
        "src/main.rs", "README.md",
    ]);
    assert!(r.success);

    // Delete the files from disk to prove switch restores them
    std::fs::remove_file(env.repo_dir.path().join("src/main.rs")).unwrap();
    std::fs::remove_file(env.repo_dir.path().join("README.md")).unwrap();
    assert!(!env.file_exists("src/main.rs"));
    assert!(!env.file_exists("README.md"));

    // Switch to the workspace
    let r = env.pulse(&["switch", &ws_id]);
    assert!(r.success, "switch failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Switched to workspace"));
    assert!(r.stdout.contains("written: 2"));

    // Files should be restored
    assert_eq!(env.read_file("src/main.rs").unwrap(), b"fn main() {}");
    assert_eq!(env.read_file("README.md").unwrap(), b"# Hello");
}

#[test]
fn switch_to_main_restores_main_state() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("merge then switch", &[]);

    // Commit and merge
    env.write_file("lib.rs", b"pub fn lib() {}");
    let r = env.pulse(&["commit", "-m", "add lib", "-w", &ws_id, "lib.rs"]);
    assert!(r.success);
    let r = env.pulse(&["merge", &ws_id]);
    assert!(r.success);

    // Delete file from disk
    std::fs::remove_file(env.repo_dir.path().join("lib.rs")).unwrap();

    // Switch to main
    let r = env.pulse(&["switch", "main"]);
    assert!(r.success, "switch failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Switched to main"));

    // File should be restored
    assert_eq!(env.read_file("lib.rs").unwrap(), b"pub fn lib() {}");
}

#[test]
fn switch_removes_extra_files() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("remove extras", &[]);

    // Commit one file
    env.write_file("keep.rs", b"keep me");
    let r = env.pulse(&["commit", "-m", "add keep", "-w", &ws_id, "keep.rs"]);
    assert!(r.success);

    // Write an extra file NOT in the snapshot
    env.write_file("extra.rs", b"delete me");
    assert!(env.file_exists("extra.rs"));

    // Switch to workspace -- extra file should be removed
    let r = env.pulse(&["switch", &ws_id]);
    assert!(r.success);
    assert!(r.stdout.contains("deleted: 1"));

    assert!(env.file_exists("keep.rs"));
    assert!(!env.file_exists("extra.rs"));
}

#[test]
fn switch_between_workspaces() {
    let env = TestEnv::new();
    env.init_repo();

    // Create workspace A with file_a
    let r = env.pulse(&["workspace", "create", "--intent", "feature A"]);
    assert!(r.success);
    let ws_a = TestEnv::extract_workspace_id(&r.stdout);
    env.write_file("file_a.rs", b"// feature A");
    let r = env.pulse(&["commit", "-m", "add A", "-w", &ws_a, "file_a.rs"]);
    assert!(r.success);

    // Create workspace B with file_b
    let r = env.pulse(&["workspace", "create", "--intent", "feature B"]);
    assert!(r.success);
    let ws_b = TestEnv::extract_workspace_id(&r.stdout);
    env.write_file("file_b.rs", b"// feature B");
    let r = env.pulse(&["commit", "-m", "add B", "-w", &ws_b, "file_b.rs"]);
    assert!(r.success);

    // Switch to A -- should have file_a only
    let r = env.pulse(&["switch", &ws_a]);
    assert!(r.success);
    assert!(env.file_exists("file_a.rs"));
    assert!(!env.file_exists("file_b.rs"));
    assert_eq!(env.read_file("file_a.rs").unwrap(), b"// feature A");

    // Switch to B -- should have file_b only
    let r = env.pulse(&["switch", &ws_b]);
    assert!(r.success);
    assert!(!env.file_exists("file_a.rs"));
    assert!(env.file_exists("file_b.rs"));
    assert_eq!(env.read_file("file_b.rs").unwrap(), b"// feature B");

    // Switch back to main -- should have neither (empty main)
    let r = env.pulse(&["switch", "main"]);
    assert!(r.success);
    assert!(!env.file_exists("file_a.rs"));
    assert!(!env.file_exists("file_b.rs"));
}

#[test]
fn commit_uses_current_workspace() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("implicit workspace", &[]);

    // Switch to the workspace
    let r = env.pulse(&["switch", &ws_id]);
    assert!(r.success);

    // Commit WITHOUT -w flag -- should use current workspace
    env.write_file("auto.rs", b"// auto commit");
    let r = env.pulse(&["commit", "-m", "auto commit", "auto.rs"]);
    assert!(r.success, "commit without -w failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Committed"));
    assert!(r.stdout.contains("auto.rs"));
}

#[test]
fn status_shows_current_workspace() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("status test", &[]);

    // Before switch -- should show main
    let r = env.pulse(&["status"]);
    assert!(r.success);
    assert!(r.stdout.contains("on:          main"));

    // Switch to workspace
    let r = env.pulse(&["switch", &ws_id]);
    assert!(r.success);

    // After switch -- should show workspace id
    let r = env.pulse(&["status"]);
    assert!(r.success);
    assert!(
        r.stdout.contains(&format!("on:          {ws_id}")),
        "expected 'on: {ws_id}' in:\n{}",
        r.stdout
    );

    // Switch back to main
    let r = env.pulse(&["switch", "main"]);
    assert!(r.success);

    let r = env.pulse(&["status"]);
    assert!(r.success);
    assert!(r.stdout.contains("on:          main"));
}

#[test]
fn switch_preserves_nested_dirs() {
    let env = TestEnv::new();
    let ws_id = env.setup_workspace("nested dirs", &[]);

    env.write_file("a/b/c/deep.rs", b"deep content");
    env.write_file("x/y/z/other.txt", b"other content");
    let r = env.pulse(&[
        "commit", "-m", "add nested", "-w", &ws_id,
        "a/b/c/deep.rs", "x/y/z/other.txt",
    ]);
    assert!(r.success);

    // Wipe everything
    let _ = std::fs::remove_dir_all(env.repo_dir.path().join("a"));
    let _ = std::fs::remove_dir_all(env.repo_dir.path().join("x"));

    // Switch restores nested structure
    let r = env.pulse(&["switch", &ws_id]);
    assert!(r.success);
    assert_eq!(env.read_file("a/b/c/deep.rs").unwrap(), b"deep content");
    assert_eq!(env.read_file("x/y/z/other.txt").unwrap(), b"other content");
}

#[test]
fn switch_main_keyword() {
    let env = TestEnv::new();
    env.init_repo();

    // "main" is a keyword, not a workspace id
    let r = env.pulse(&["switch", "main"]);
    assert!(r.success, "switch main failed: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("Switched to main"));
}
