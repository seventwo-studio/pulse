use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl Config {
    fn path(repo_root: &Path) -> std::path::PathBuf {
        repo_root.join(".pulse").join("config.json")
    }

    pub fn load(repo_root: &Path) -> anyhow::Result<Self> {
        let path = Self::path(repo_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn save(&self, repo_root: &Path) -> anyhow::Result<()> {
        let path = Self::path(repo_root);
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&path, data)?;
        Ok(())
    }
}
