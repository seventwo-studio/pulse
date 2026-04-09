mod client;
mod config;
mod core;
mod storage;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::{Parser, Subcommand};

use client::http::{PulseClient, SyncBundle};
use config::Config;
use core::diff::diff_snapshots;
use core::merge::{MergeEngine, MergeResult};
use core::overlap::detect_scope_overlaps;
use core::primitives::*;
use core::trunk::TrunkManager;
use core::workspace::WorkspaceManager;
use storage::engine::StorageEngine;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "pulse", about = "AI-native version control")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new repository
    Init {
        /// Remote server URL for sync
        #[arg(long)]
        remote: Option<String>,
    },
    /// Workspace operations
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// Commit files to a workspace
    Commit {
        /// Commit message
        #[arg(long, short)]
        message: String,
        /// Workspace ID
        #[arg(long, short)]
        workspace: String,
        /// Files to commit
        files: Vec<PathBuf>,
    },
    /// Merge a workspace into trunk
    Merge {
        /// Workspace ID
        workspace: String,
    },
    /// Show trunk history
    Log {
        #[arg(long)]
        author: Option<String>,
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Show file contents from trunk
    Show {
        /// File path to retrieve
        path: String,
    },
    /// List files in the trunk snapshot
    Files,
    /// Compare two snapshots or changesets
    Diff {
        a: String,
        b: String,
    },
    /// Show repository status
    Status,
}

#[derive(Subcommand)]
enum WorkspaceAction {
    /// Create a new workspace
    Create {
        #[arg(long)]
        intent: String,
        #[arg(long)]
        scope: Vec<String>,
    },
    /// List workspaces
    List {
        #[arg(long)]
        all: bool,
    },
    /// Show workspace details
    Status {
        id: String,
    },
    /// Abandon a workspace
    Abandon {
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Walk up from CWD to find the repo root (directory containing `.pulse/`).
fn find_repo_root() -> anyhow::Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join(".pulse").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            anyhow::bail!("not a pulse repository (or any parent)");
        }
    }
}

/// Open the storage engine for an existing repository.
fn open_storage() -> anyhow::Result<StorageEngine> {
    let root = find_repo_root()?;
    Ok(StorageEngine::open(root)?)
}

fn current_author() -> Author {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".into());
    Author::human(user)
}

fn short_hash(hash: &Hash) -> String {
    hash.to_string()[..8].to_string()
}

/// Resolve a hash string as a changeset or snapshot.
fn resolve_snapshot(engine: &StorageEngine, hash_str: &str) -> anyhow::Result<Snapshot> {
    let hash: Hash = hash_str
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid hash: {e}"))?;
    // Try as changeset first
    if let Ok(cs) = engine.get_changeset(&hash) {
        return Ok(engine.get_snapshot(&cs.snapshot)?.clone());
    }
    // Try as snapshot
    Ok(engine.get_snapshot(&hash)?.clone())
}

// ---------------------------------------------------------------------------
// Auto-sync
// ---------------------------------------------------------------------------

/// Try to load the remote URL. Returns None if no repo or no remote configured.
fn try_remote_url() -> Option<String> {
    let root = find_repo_root().ok()?;
    let config = Config::load(&root).ok()?;
    config.remote
}

/// Pull from remote before a command. Silently skips if no remote or network error.
async fn auto_pull(engine: &mut StorageEngine) {
    let url = match try_remote_url() {
        Some(u) => u,
        None => return,
    };
    let client = PulseClient::new(&url);
    let local_trunk = TrunkManager::head_id(engine).ok().flatten();
    match client.sync_pull(local_trunk.as_ref()).await {
        Ok(bundle) => {
            if !bundle.changesets.is_empty() {
                let count = bundle.changesets.len();
                if import_pull_bundle(engine, bundle).is_ok() {
                    eprintln!("(synced: pulled {count} changeset(s))");
                }
            }
        }
        Err(_) => {} // offline — that's fine
    }
}

/// Push to remote after a mutating command. Silently skips if no remote or network error.
async fn auto_push(engine: &StorageEngine) {
    let url = match try_remote_url() {
        Some(u) => u,
        None => return,
    };
    let client = PulseClient::new(&url);
    match build_push_bundle(engine) {
        Ok(bundle) => {
            if client.sync_push(&bundle).await.is_ok() {
                eprintln!("(synced: pushed)");
            }
        }
        Err(_) => {}
    }
}

// ---------------------------------------------------------------------------
// Sync helpers
// ---------------------------------------------------------------------------

/// Build a sync bundle from local storage (all objects).
fn build_push_bundle(engine: &StorageEngine) -> anyhow::Result<SyncBundle> {
    let trunk_id = TrunkManager::head_id(engine)?
        .ok_or_else(|| anyhow::anyhow!("repository not initialized"))?;

    // Walk the trunk chain to collect changesets in order (oldest first).
    let mut changesets = Vec::new();
    let mut current = Some(trunk_id);
    while let Some(id) = current {
        let cs = engine.get_changeset(&id)?.clone();
        current = cs.parent;
        changesets.push(cs);
    }
    changesets.reverse();

    // Collect snapshots and file content.
    let mut snapshots = Vec::new();
    let mut files: HashMap<String, String> = HashMap::new();

    for cs in &changesets {
        if let Ok(snap) = engine.get_snapshot(&cs.snapshot) {
            let snap = snap.clone();
            for (_path, blob_hash) in &snap.files {
                let hex = blob_hash.to_string();
                if !files.contains_key(&hex) {
                    if let Ok(content) = engine.read_file_by_path(&snap.id, _path) {
                        files.insert(hex, STANDARD.encode(&content));
                    }
                }
            }
            snapshots.push(snap);
        }
    }

    // Dedup snapshots
    let mut seen = std::collections::HashSet::new();
    snapshots.retain(|s| seen.insert(s.id));

    // Collect workspaces
    let workspaces: Vec<Workspace> = engine
        .list_workspaces(true)
        .into_iter()
        .cloned()
        .collect();

    Ok(SyncBundle {
        trunk: trunk_id,
        changesets,
        snapshots,
        workspaces,
        files,
    })
}

/// Import a sync bundle into local storage.
fn import_pull_bundle(engine: &mut StorageEngine, bundle: SyncBundle) -> anyhow::Result<()> {
    // Store file content first (so blobs exist for snapshots).
    for (blob_hex, b64_content) in &bundle.files {
        let content = STANDARD
            .decode(b64_content)
            .map_err(|e| anyhow::anyhow!("invalid base64 for blob {blob_hex}: {e}"))?;
        // store_file chunks and stores the content; the blob hash should match.
        engine.store_file(&content)?;
    }

    // Store snapshots.
    for snapshot in &bundle.snapshots {
        engine.store_snapshot(snapshot)?;
    }

    // Store changesets (in order — oldest first).
    for changeset in &bundle.changesets {
        engine.store_changeset(changeset)?;
    }

    // Store workspaces.
    for workspace in &bundle.workspaces {
        engine.store_workspace(workspace)?;
    }

    // Update trunk.
    engine.set_trunk(&bundle.trunk)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { remote } => {
            let cwd = std::env::current_dir()?;
            let mut engine = StorageEngine::init(&cwd)?;
            let root = TrunkManager::init_repo(&mut engine)?;
            if let Some(url) = &remote {
                let config = Config {
                    remote: Some(url.clone()),
                };
                config.save(&cwd)?;
            }
            println!("Initialized Pulse repository in .pulse/");
            println!("  changeset: {}", short_hash(&root.id));
            if let Some(url) = &remote {
                println!("  remote:    {url}");
                // If the remote already has data, pull it (clone behavior).
                // Otherwise push our fresh root.
                let client = PulseClient::new(url);
                match client.sync_pull(None).await {
                    Ok(bundle) if !bundle.changesets.is_empty() => {
                        let count = bundle.changesets.len();
                        import_pull_bundle(&mut engine, bundle)?;
                        println!("  cloned:    {count} changeset(s) from remote");
                    }
                    _ => {
                        // Remote is empty or unreachable — push our root
                        if let Ok(bundle) = build_push_bundle(&engine) {
                            let _ = client.sync_push(&bundle).await;
                        }
                    }
                }
            }
        }

        Commands::Status => {
            let mut engine = open_storage()?;
            auto_pull(&mut engine).await;
            match TrunkManager::head(&engine)? {
                Some(cs) => {
                    println!("trunk:       {}", short_hash(&cs.id));
                    println!("last commit: {}", cs.message);
                    println!("last author: {}", cs.author.id);
                    println!(
                        "timestamp:   {}",
                        cs.timestamp.format("%Y-%m-%d %H:%M:%S")
                    );
                }
                None => {
                    println!("Empty repository (no trunk).");
                }
            }
            let active = WorkspaceManager::list(&engine, false);
            println!("workspaces:  {} active", active.len());

            let root = find_repo_root()?;
            let config = Config::load(&root)?;
            match config.remote {
                Some(url) => println!("remote:      {url}"),
                None => println!("remote:      (none)"),
            }
        }

        Commands::Log { author, limit } => {
            let mut engine = open_storage()?;
            auto_pull(&mut engine).await;
            let changesets =
                TrunkManager::log(&engine, limit, author.as_deref(), None)?;
            if changesets.is_empty() {
                println!("No changesets.");
            } else {
                for cs in &changesets {
                    println!(
                        "{} {} ({}, {})",
                        short_hash(&cs.id),
                        cs.message,
                        cs.author.id,
                        cs.timestamp.format("%Y-%m-%d %H:%M:%S"),
                    );
                }
            }
        }

        Commands::Workspace { action } => match action {
            WorkspaceAction::Create { intent, scope } => {
                let mut engine = open_storage()?;
                auto_pull(&mut engine).await;
                let head = TrunkManager::head_id(&engine)?
                    .ok_or_else(|| anyhow::anyhow!("repository not initialized"))?;
                let author = current_author();
                let ws = WorkspaceManager::create(
                    &mut engine,
                    intent,
                    scope,
                    author,
                    &head,
                )?;

                // Detect scope overlaps with other active workspaces.
                let others = WorkspaceManager::list(&engine, false);
                let overlaps = detect_scope_overlaps(&ws, &others);

                println!("Created workspace {}", ws.id);
                println!("  intent: {}", ws.intent);
                println!("  scope:  {:?}", ws.scope);
                println!("  base:   {}", short_hash(&ws.base));
                if !overlaps.is_empty() {
                    println!("  overlaps: {} detected", overlaps.len());
                    for o in &overlaps {
                        println!("    with {}: {:?}", o.workspace_b, o.overlapping_scopes);
                    }
                }
                auto_push(&engine).await;
            }
            WorkspaceAction::List { all } => {
                let mut engine = open_storage()?;
                auto_pull(&mut engine).await;
                let workspaces = WorkspaceManager::list(&engine, all);
                if workspaces.is_empty() {
                    println!("No workspaces.");
                } else {
                    println!("{:<10} {:<30} {}", "ID", "INTENT", "STATUS");
                    for ws in &workspaces {
                        let status = format!("{:?}", ws.status).to_lowercase();
                        println!("{:<10} {:<30} {}", ws.id, ws.intent, status);
                    }
                }
            }
            WorkspaceAction::Status { id } => {
                let mut engine = open_storage()?;
                auto_pull(&mut engine).await;
                let ws = WorkspaceManager::get(&engine, &id)?;
                let status = format!("{:?}", ws.status).to_lowercase();
                println!("Workspace {}", ws.id);
                println!("  status:     {status}");
                println!("  intent:     {}", ws.intent);
                println!("  scope:      {:?}", ws.scope);
                println!(
                    "  author:     {} ({:?})",
                    ws.author.id,
                    ws.author.kind
                );
                println!("  base:       {}", short_hash(&ws.base));
                println!("  changesets: {}", ws.changesets.len());
            }
            WorkspaceAction::Abandon { id } => {
                let mut engine = open_storage()?;
                auto_pull(&mut engine).await;
                let ws = WorkspaceManager::abandon(&mut engine, &id)?;
                println!("Abandoned workspace {}", ws.id);
                auto_push(&engine).await;
            }
        },

        Commands::Commit {
            message,
            workspace,
            files,
        } => {
            let mut engine = open_storage()?;
            auto_pull(&mut engine).await;
            let author = current_author();

            let file_data: Vec<(String, Vec<u8>)> = files
                .iter()
                .map(|path| {
                    let bytes = std::fs::read(path)
                        .map_err(|e| anyhow::anyhow!("failed to read {}: {}", path.display(), e))?;
                    let key = path.to_string_lossy().to_string();
                    Ok((key, bytes))
                })
                .collect::<anyhow::Result<_>>()?;

            let result =
                WorkspaceManager::commit(&mut engine, &workspace, file_data, message, author)?;
            println!("Committed {}", short_hash(&result.changeset.id));
            println!(
                "  new chunks:    {}\n  reused chunks: {}",
                result.stats.new_chunks, result.stats.reused_chunks,
            );
            if !result.changeset.files_changed.is_empty() {
                println!(
                    "  files: {}",
                    result.changeset.files_changed.join(", ")
                );
            }
            auto_push(&engine).await;
        }

        Commands::Merge { workspace } => {
            let mut engine = open_storage()?;
            auto_pull(&mut engine).await;
            let result = MergeEngine::merge(&mut engine, &workspace)?;

            match result {
                MergeResult::Success { changeset } => {
                    println!(
                        "Merged into trunk. Changeset: {}",
                        short_hash(&changeset.id)
                    );
                    if !changeset.files_changed.is_empty() {
                        println!("  files: {}", changeset.files_changed.join(", "));
                    }
                }
                MergeResult::Conflict {
                    conflicting_files,
                    trunk_snapshot,
                    workspace_snapshot,
                } => {
                    println!("Merge conflict!");
                    for f in &conflicting_files {
                        println!("  conflict: {f}");
                    }
                    println!(
                        "  trunk snapshot:     {}",
                        short_hash(&trunk_snapshot)
                    );
                    println!(
                        "  workspace snapshot: {}",
                        short_hash(&workspace_snapshot)
                    );
                }
            }
            auto_push(&engine).await;
        }

        Commands::Show { path } => {
            let mut engine = open_storage()?;
            auto_pull(&mut engine).await;
            let snap = TrunkManager::snapshot(&engine)?
                .ok_or_else(|| anyhow::anyhow!("no trunk snapshot"))?;
            let bytes = engine.read_file_by_path(&snap.id, &path)?;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&bytes)?;
        }

        Commands::Files => {
            let mut engine = open_storage()?;
            auto_pull(&mut engine).await;
            let snap = TrunkManager::snapshot(&engine)?
                .ok_or_else(|| anyhow::anyhow!("no trunk snapshot"))?;
            if snap.files.is_empty() {
                println!("No files in trunk.");
            } else {
                for (path, hash) in &snap.files {
                    println!("{}  {}", short_hash(hash), path);
                }
            }
        }

        Commands::Diff { a, b } => {
            let mut engine = open_storage()?;
            auto_pull(&mut engine).await;
            let snap_a = resolve_snapshot(&engine, &a)?;
            let snap_b = resolve_snapshot(&engine, &b)?;
            let diff = diff_snapshots(&snap_a, &snap_b);

            if diff.is_empty() {
                println!("No differences.");
            } else {
                for path in &diff.added {
                    println!("+ added:    {path}");
                }
                for path in &diff.removed {
                    println!("- removed:  {path}");
                }
                for path in &diff.modified {
                    println!("~ modified: {path}");
                }
            }
        }

    }

    Ok(())
}
