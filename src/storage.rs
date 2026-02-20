use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{config::StorageConfig, types::ProcessedProposal};

#[derive(Debug, Clone)]
pub struct Storage {
    state_path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub last_scanned_block: u64,
    pub proposals: BTreeMap<String, ProcessedProposal>,
}

impl Storage {
    pub fn new(cfg: &StorageConfig) -> Result<Self> {
        fs::create_dir_all(&cfg.data_dir).with_context(|| {
            format!("failed to create data directory {}", cfg.data_dir.display())
        })?;
        Ok(Self {
            state_path: cfg.data_dir.join(&cfg.state_file),
        })
    }

    pub fn state_path(&self) -> &PathBuf {
        &self.state_path
    }

    pub fn load(&self) -> Result<State> {
        if !self.state_path.exists() {
            return Ok(State::default());
        }

        let raw = fs::read_to_string(&self.state_path)
            .with_context(|| format!("failed to read {}", self.state_path.display()))?;
        let parsed: State = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", self.state_path.display()))?;
        Ok(parsed)
    }

    pub fn save(&self, state: &State) -> Result<()> {
        let mut tmp = self.state_path.clone();
        tmp.set_extension("json.tmp");

        let data = serde_json::to_vec_pretty(state)?;
        fs::write(&tmp, data).with_context(|| format!("failed to write {}", tmp.display()))?;
        fs::rename(&tmp, &self.state_path).with_context(|| {
            format!(
                "failed to move {} to {}",
                tmp.display(),
                self.state_path.display()
            )
        })?;
        Ok(())
    }
}
