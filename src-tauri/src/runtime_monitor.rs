use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::Utc;
use serde::Serialize;

use crate::build_info::{BuildConsistencyInfo, RuntimeSecurityInfo};

const DEFAULT_LOG_CAPACITY: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeLogEntry {
    pub timestamp: String,
    pub level: String,
    pub scope: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Default)]
struct RuntimeMonitorState {
    entries: VecDeque<RuntimeLogEntry>,
    build: Option<BuildConsistencyInfo>,
    security: Option<RuntimeSecurityInfo>,
}

pub struct RuntimeMonitor {
    inner: Mutex<RuntimeMonitorState>,
}

impl Default for RuntimeMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeMonitor {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RuntimeMonitorState::default()),
        }
    }

    pub fn record(
        &self,
        level: &str,
        scope: &str,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) {
        let mut guard = self.inner.lock().unwrap();
        guard.entries.push_back(RuntimeLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            level: level.to_string(),
            scope: scope.to_string(),
            message: message.into(),
            data,
        });
        while guard.entries.len() > DEFAULT_LOG_CAPACITY {
            guard.entries.pop_front();
        }
    }

    pub fn recent_entries(&self, limit: usize) -> Vec<RuntimeLogEntry> {
        let guard = self.inner.lock().unwrap();
        let count = if limit == 0 {
            guard.entries.len()
        } else {
            limit.min(guard.entries.len())
        };
        guard
            .entries
            .iter()
            .rev()
            .take(count)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    pub fn set_build_consistency(&self, info: BuildConsistencyInfo) {
        self.inner.lock().unwrap().build = Some(info);
    }

    pub fn build_consistency(&self) -> Option<BuildConsistencyInfo> {
        self.inner.lock().unwrap().build.clone()
    }

    pub fn set_security(&self, info: RuntimeSecurityInfo) {
        self.inner.lock().unwrap().security = Some(info);
    }

    pub fn security(&self) -> Option<RuntimeSecurityInfo> {
        self.inner.lock().unwrap().security.clone()
    }
}
