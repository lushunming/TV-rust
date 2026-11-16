//! 热搜榜数据：对齐 composeApp `bean/Hot.kt`
//!
//! 用途：首页顶栏搜索框的轮播提示（对应原实现 `VideoViewModel.searchBarPrompt()`）。
//! 缓存与 Kotlin 共用 `data/cache/hot/hotlist.json`，两端可互相复用。

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::core::config::CHROME_UA;
use crate::core::paths;

/// 热搜条目（字段与原 Kotlin `HotData` 一致）
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HotData {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub upinfo: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct HotEnvelope {
    #[serde(default)]
    data: Vec<HotData>,
}

const HOT_URL: &str = "https://api.web.360kan.com/v1/rank?cat=1";
/// 360 榜单接口要求带 Referer，否则按盗链拒绝
const HOT_REFERER: &str = "https://www.360kan.com/rank/general";

fn cache_file() -> std::path::PathBuf {
    paths::ensure_dir(&paths::hot_cache()).join("hotlist.json")
}

/// 读取磁盘缓存；缺失或损坏时返回空列表（不报错，避免启动噪音）
pub fn load_cache() -> Vec<HotData> {
    let Ok(text) = std::fs::read_to_string(cache_file()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<HotData>>(&text)
        .unwrap_or_default()
        .into_iter()
        .filter(|h| !h.title.trim().is_empty())
        .collect()
}

fn save_cache(list: &[HotData]) {
    if let Ok(text) = serde_json::to_string(list) {
        let _ = std::fs::write(cache_file(), text);
    }
}

/// 阻塞拉取榜单；成功且非空时写缓存
pub fn fetch() -> Result<Vec<HotData>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(CHROME_UA)
        .build()?;
    let resp = client.get(HOT_URL).header("Referer", HOT_REFERER).send()?;
    if !resp.status().is_success() {
        return Err(anyhow!("热搜接口返回 {}", resp.status()));
    }
    let env: HotEnvelope = serde_json::from_str(&resp.text()?)?;
    let list: Vec<HotData> = env
        .data
        .into_iter()
        .filter(|h| !h.title.trim().is_empty())
        .collect();
    if !list.is_empty() {
        save_cache(&list);
    }
    Ok(list)
}
