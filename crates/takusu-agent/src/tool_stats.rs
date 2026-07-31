use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolStat {
    pub count: u64,
    pub error_count: u64,
    pub last_used: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolStatsSnapshot {
    pub tools: HashMap<String, ToolStat>,
}

pub struct ToolStats {
    path: Option<PathBuf>,
    inner: Mutex<ToolStatsSnapshot>,
}

// Process-wide shared instance. Tool stats are global (backed by a single
// file), not per-session; sharing one accumulator across all sessions in a
// process prevents concurrent sessions from clobbering each other's counts
// when they flush.
static SHARED: OnceLock<Arc<ToolStats>> = OnceLock::new();

impl ToolStats {
    /// The process-wide shared instance, loaded from disk on first access.
    /// All agent sessions and the transport stats endpoints use this.
    pub fn shared() -> Arc<ToolStats> {
        SHARED.get_or_init(|| Arc::new(ToolStats::load())).clone()
    }

    /// Load a fresh snapshot from disk. Used by the CLI to display or clear
    /// stats from a separate process.
    ///
    /// This does not touch the in-memory state of a running server: a server
    /// process holds its own [`ToolStats::shared`] accumulator, and its next
    /// flush will rewrite the file from that (stale, relative to this call)
    /// state. Cross-process clears are therefore eventually-consistent, not
    /// immediate.
    pub fn load() -> Self {
        let path = stats_path();
        let inner = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    pub fn record(&self, tool_name: &str, is_error: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.tools.entry(tool_name.to_string()).or_default();
        entry.count += 1;
        if is_error {
            entry.error_count += 1;
        }
        entry.last_used = Some(jiff::Timestamp::now().to_string());
    }

    pub fn snapshot(&self) -> ToolStatsSnapshot {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn clear(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.tools.clear();
        self.save_inner(&guard);
    }

    pub fn flush(&self) {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        self.save_inner(&guard);
    }

    fn save_inner(&self, snapshot: &ToolStatsSnapshot) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(snapshot) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn stats_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let mut p = PathBuf::from(h);
                p.push(".local");
                p.push("state");
                p
            })
        })?;
    Some(base.join("takusu/agent/tool_stats.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_increments_counts() {
        let stats = ToolStats {
            path: None,
            inner: Mutex::new(ToolStatsSnapshot::default()),
        };
        stats.record("get_task", false);
        stats.record("get_task", true);
        stats.record("list_tasks", false);

        let snap = stats.snapshot();
        let get_task = &snap.tools["get_task"];
        assert_eq!(get_task.count, 2);
        assert_eq!(get_task.error_count, 1);
        assert!(get_task.last_used.is_some());

        let list_tasks = &snap.tools["list_tasks"];
        assert_eq!(list_tasks.count, 1);
        assert_eq!(list_tasks.error_count, 0);
    }

    #[test]
    fn clear_removes_all() {
        let stats = ToolStats {
            path: None,
            inner: Mutex::new(ToolStatsSnapshot::default()),
        };
        stats.record("get_task", false);
        stats.clear();
        assert!(stats.snapshot().tools.is_empty());
    }

    #[test]
    fn snapshot_round_trips_json() {
        let stats = ToolStats {
            path: None,
            inner: Mutex::new(ToolStatsSnapshot::default()),
        };
        stats.record("create_task", false);
        let snap = stats.snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let parsed: ToolStatsSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tools["create_task"].count, 1);
    }
}
