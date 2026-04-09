mod client;
mod core;
mod storage;

use std::collections::HashMap;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use client::buffer::OfflineBuffer;
use client::http::PulseClient;
use core::primitives::Author;

#[derive(Parser)]
#[command(name = "pulse", about = "AI-native version control")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a repository
    Init {
        /// Remote server URL
        #[arg(long)]
        remote: Option<String>,
        /// Use local mode (embedded server)
        #[arg(long)]
        local: bool,
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
    /// Compare two snapshots
    Diff {
        a: String,
        b: String,
    },
    /// Transfer repository to another Pulse server
    Transfer {
        /// Target server URL (e.g. https://pulse.example.com)
        target: String,
        /// Source server URL (defaults to PULSE_URL or localhost:3000)
        #[arg(long)]
        source: Option<String>,
    },
    /// Show repository status
    Status,
    /// Offline commit queue
    Queue {
        #[command(subcommand)]
        action: QueueAction,
    },
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

#[derive(Subcommand)]
enum QueueAction {
    /// Show buffered commit count
    Status,
    /// Force replay buffered commits to the server
    Drain,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_url() -> String {
    std::env::var("PULSE_URL").unwrap_or_else(|_| "http://localhost:3000".into())
}

fn current_author() -> Author {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".into());
    Author::human(user)
}

/// Format a hash as an 8-character short form.
fn short_hash(hash: &crate::core::primitives::Hash) -> String {
    hash.to_string()[..8].to_string()
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { remote, local: _ } => {
            let url = remote.unwrap_or_else(default_url);
            let client = PulseClient::new(&url);
            let resp = client.init_repo().await?;
            println!(
                "Repository initialized.\n  changeset: {}\n  snapshot:  {}",
                short_hash(&resp.changeset_id),
                short_hash(&resp.snapshot_id),
            );
        }

        Commands::Status => {
            let client = PulseClient::new(&default_url());
            let resp = client.status().await?;
            println!("trunk:             {}", short_hash(&resp.trunk));
            println!("active workspaces: {}", resp.active_workspaces);
        }

        Commands::Log { author, limit } => {
            let client = PulseClient::new(&default_url());
            let changesets = client.trunk_log(Some(limit), author.as_deref()).await?;
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
                let client = PulseClient::new(&default_url());
                let author = current_author();
                let resp = client.create_workspace(&intent, &scope, &author).await?;
                let ws = &resp.workspace;
                println!("Created workspace {}", ws.id);
                println!("  intent: {}", ws.intent);
                println!("  scope:  {:?}", ws.scope);
                println!("  base:   {}", short_hash(&ws.base));
                if !resp.overlaps.is_empty() {
                    println!("  overlaps: {} detected", resp.overlaps.len());
                }
            }
            WorkspaceAction::List { all } => {
                let client = PulseClient::new(&default_url());
                let workspaces = client.list_workspaces(all).await?;
                if workspaces.is_empty() {
                    println!("No workspaces.");
                } else {
                    println!("{:<10} {:<30} {}", "ID", "INTENT", "STATUS");
                    for ws in &workspaces {
                        let status = serde_json::to_value(&ws.status)
                            .ok()
                            .and_then(|v| v.as_str().map(String::from))
                            .unwrap_or_else(|| format!("{:?}", ws.status));
                        println!("{:<10} {:<30} {}", ws.id, ws.intent, status);
                    }
                }
            }
            WorkspaceAction::Status { id } => {
                let client = PulseClient::new(&default_url());
                let ws = client.get_workspace(&id).await?;
                let status = serde_json::to_value(&ws.status)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| format!("{:?}", ws.status));
                println!("Workspace {}", ws.id);
                println!("  status:     {}", status);
                println!("  intent:     {}", ws.intent);
                println!("  scope:      {:?}", ws.scope);
                println!("  author:     {} ({})", ws.author.id, format!("{:?}", ws.author.kind).to_lowercase());
                println!("  base:       {}", short_hash(&ws.base));
                println!("  changesets: {}", ws.changesets.len());
            }
            WorkspaceAction::Abandon { id } => {
                let client = PulseClient::new(&default_url());
                let ws = client.abandon_workspace(&id).await?;
                println!("Abandoned workspace {}", ws.id);
            }
        },

        Commands::Commit {
            message,
            workspace,
            files,
        } => {
            let client = PulseClient::new(&default_url());
            let author = current_author();

            let mut file_map: HashMap<String, Vec<u8>> = HashMap::new();
            for path in &files {
                let bytes = std::fs::read(path)
                    .map_err(|e| anyhow::anyhow!("failed to read {}: {}", path.display(), e))?;
                let key = path.to_string_lossy().to_string();
                file_map.insert(key, bytes);
            }

            let resp = client.commit(&workspace, file_map, &message, &author).await?;
            println!("Committed {}", short_hash(&resp.changeset.id));
            println!(
                "  new chunks:    {}\n  reused chunks: {}",
                resp.stats.new_chunks, resp.stats.reused_chunks,
            );
            if !resp.changeset.files_changed.is_empty() {
                println!("  files: {}", resp.changeset.files_changed.join(", "));
            }
        }

        Commands::Merge { workspace } => {
            let client = PulseClient::new(&default_url());
            let result = client.merge(&workspace).await?;

            if let Some(cs) = result.get("changeset") {
                let id = cs.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let short = &id[..id.len().min(8)];
                println!("Merged into trunk. Changeset: {short}");
            } else if let Some(files) = result.get("conflicting_files") {
                println!("Merge conflict!");
                if let Some(arr) = files.as_array() {
                    for f in arr {
                        if let Some(s) = f.as_str() {
                            println!("  conflict: {s}");
                        }
                    }
                }
                if let Some(ts) = result.get("trunk_snapshot").and_then(|v| v.as_str()) {
                    println!("  trunk snapshot:    {}", &ts[..ts.len().min(8)]);
                }
                if let Some(ws) = result.get("workspace_snapshot").and_then(|v| v.as_str()) {
                    println!("  workspace snapshot: {}", &ws[..ws.len().min(8)]);
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }

        Commands::Queue { action } => {
            let buffer_path = OfflineBuffer::default_path()?;
            let buffer = OfflineBuffer::new(buffer_path);

            match action {
                QueueAction::Status => {
                    let count = buffer.len()?;
                    if count == 0 {
                        println!("Offline queue is empty.");
                    } else {
                        println!("Offline queue: {count} buffered commit(s).");
                    }
                }
                QueueAction::Drain => {
                    let entries = buffer.drain()?;
                    if entries.is_empty() {
                        println!("Nothing to replay.");
                        return Ok(());
                    }

                    let client = PulseClient::new(&default_url());
                    println!("Replaying {} buffered commit(s)...", entries.len());

                    for (i, entry) in entries.iter().enumerate() {
                        let files: HashMap<String, Vec<u8>> = entry
                            .files
                            .iter()
                            .map(|(path, b64)| {
                                let bytes = STANDARD.decode(b64).unwrap_or_default();
                                (path.clone(), bytes)
                            })
                            .collect();

                        match client
                            .commit(&entry.workspace_id, files, &entry.message, &entry.author)
                            .await
                        {
                            Ok(resp) => {
                                println!(
                                    "  [{}/{}] {} committed as {}",
                                    i + 1,
                                    entries.len(),
                                    entry.message,
                                    short_hash(&resp.changeset.id),
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "  [{}/{}] {} failed: {e}",
                                    i + 1,
                                    entries.len(),
                                    entry.message,
                                );
                            }
                        }
                    }
                }
            }
        }

        Commands::Diff { a, b } => {
            let client = PulseClient::new(&default_url());
            let diff = client.diff(&a, &b).await?;

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

        Commands::Transfer { target, source } => {
            let source_url = source.unwrap_or_else(default_url);
            let client = PulseClient::new(&source_url);
            let result = client.transfer(&target).await?;
            println!("Transfer complete.");
            println!("  target:     {}", result.target_url);
            println!("  blobs:      {}", result.exported.blobs);
            println!("  snapshots:  {}", result.exported.snapshots);
            println!("  changesets: {}", result.exported.changesets);
            println!("  workspaces: {}", result.exported.workspaces);
            if let Some(imported) = result.import_result.get("imported") {
                if let Some(trunk_set) = imported.get("trunk_set").and_then(|v| v.as_bool()) {
                    println!("  trunk set:  {}", trunk_set);
                }
            }
        }
    }

    Ok(())
}
