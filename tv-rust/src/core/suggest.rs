//! 搜索联想：对齐 composeApp `ui/search/SearchBar.kt` 里对 iqiyi suggest 的调用。
//!
//! 原实现：输入变化后 500ms 防抖，GET
//! `https://suggest.video.iqiyi.com/?if=mobile&key=<关键词>`，取 `data[].name` 做下拉项。
//! 返回体形如：{"code":"A00000","data":[{"name":"红海行动",...}]}

use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use crate::core::config::CHROME_UA;

const SUGGEST_URL: &str = "https://suggest.video.iqiyi.com/?if=mobile&key=";
/// 下拉最多展示条数
const MAX_ITEMS: usize = 8;

#[derive(Debug, Deserialize)]
struct SuggestItem {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct SuggestEnvelope {
    #[serde(default)]
    data: Vec<SuggestItem>,
}

/// 拉取联想词；去重、去空、上限 8 条。失败由调用方决定是否仅记日志。
pub fn fetch(key: &str) -> Result<Vec<String>> {
    let key = key.trim();
    if key.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(CHROME_UA)
        .build()?;
    let url = format!("{SUGGEST_URL}{}", urlencode(key));
    let text = client.get(url).send()?.error_for_status()?.text()?;
    let env: SuggestEnvelope = serde_json::from_str(&text)?;

    let mut out: Vec<String> = Vec::new();
    for item in env.data {
        let name = item.name.trim().to_string();
        if name.is_empty() || out.iter().any(|x| x == &name) {
            continue;
        }
        out.push(name);
        if out.len() >= MAX_ITEMS {
            break;
        }
    }
    Ok(out)
}

/// 最小 percent-encoding（只保留 unreserved 字符），避免为一次请求引入额外依赖
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
