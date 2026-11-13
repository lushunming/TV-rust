//! 观看历史（Rust 侧自有存储，JSON 文件，不与 Kotlin 的 Room DB 混用）。
//! key 约定与原应用一致：siteKey@@@vodId@@@cfgId

use crate::core::paths;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub key: String,
    pub site_key: String,
    pub vod_id: String,
    pub cfg_id: String,
    pub vod_name: String,
    pub vod_pic: String,
    pub vod_flag: String,
    pub ep_name: String,
    pub episode_url: String,
    pub create_time: i64, // 毫秒
}

pub struct HistoryStore {
    inner: Mutex<Vec<HistoryEntry>>, // 按时间倒序，最新的在前
    path: std::path::PathBuf,
}

impl HistoryStore {
    pub fn load() -> Self {
        let dir = paths::ensure_dir(&paths::rust_data());
        let path = dir.join("history.json");
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<HistoryEntry>>(&s).ok())
            .unwrap_or_default();
        let mut entries = entries;
        entries.sort_by(|a, b| b.create_time.cmp(&a.create_time));
        HistoryStore { inner: Mutex::new(entries), path }
    }

    pub fn list(&self) -> Vec<HistoryEntry> {
        self.inner.lock().unwrap().clone()
    }

    /// 记录/更新一条历史（同 key 上移到最前并刷新时间）
    pub fn upsert(&self, entry: HistoryEntry) {
        let mut list = self.inner.lock().unwrap();
        list.retain(|e| e.key != entry.key);
        list.insert(0, entry);
        list.truncate(500);
        drop(list);
        self.save();
    }

    pub fn remove(&self, key: &str) {
        self.inner.lock().unwrap().retain(|e| e.key != key);
        self.save();
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
        self.save();
    }

    fn save(&self) {
        let list = self.inner.lock().unwrap().clone();
        if let Ok(text) = serde_json::to_string_pretty(&list) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}
