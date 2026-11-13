//! 配置源获取：http（带 ETag/Last-Modified 磁盘缓存，与 Kotlin 共用缓存目录）、file、内联 JSON。
//! 对应 Kotlin ApiConfig.getData / fetchConfigWithCache。

use crate::core::paths;
use anyhow::{anyhow, Result};
use md5::{Digest, Md5};
use std::path::PathBuf;
use std::time::Duration;

pub const CHROME_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    out.iter().map(|b| format!("{:02x}", b)).collect()
}

pub enum SourceKind {
    Http(String),
    File(PathBuf),
    Raw,
}

pub fn classify(source: &str) -> SourceKind {
    let t = source.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        return SourceKind::Http(t.to_string());
    }
    if t.starts_with("file:") {
        let p = t.replace("file://", "").replace("file:/", "");
        return SourceKind::File(PathBuf::from(p));
    }
    // 以 { 开头视为内联 JSON
    if t.starts_with('{') {
        return SourceKind::Raw;
    }
    // 其余当作本地文件路径
    SourceKind::File(PathBuf::from(t))
}

struct CacheMeta {
    etag: Option<String>,
    last_modified: Option<String>,
}

fn read_meta(meta: &PathBuf) -> CacheMeta {
    match std::fs::read_to_string(meta) {
        Ok(text) => {
            let mut lines = text.lines();
            CacheMeta {
                etag: lines.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                last_modified: lines.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            }
        }
        Err(_) => CacheMeta { etag: None, last_modified: None },
    }
}

fn fetch_http_with_cache(url: &str) -> Result<String> {
    let key = md5_hex(url);
    let dir = paths::ensure_dir(&paths::config_cache());
    let cache_file = dir.join(format!("{}.json", key));
    let meta_file = dir.join(format!("{}.meta", key));
    let cached = std::fs::read_to_string(&cache_file)
        .ok()
        .filter(|s| !s.trim().is_empty());
    let meta = read_meta(&meta_file);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(CHROME_UA)
        .build()?;

    let mut req = client.get(url);
    if let Some(etag) = &meta.etag {
        req = req.header("If-None-Match", etag);
    }
    if let Some(lm) = &meta.last_modified {
        req = req.header("If-Modified-Since", lm);
    }

    match req.send() {
        Ok(resp) => {
            let status = resp.status();
            // 先取头部（resp.text() 会消费 resp）
            let etag = resp
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let lm = resp
                .headers()
                .get(reqwest::header::LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if status.as_u16() == 304 {
                if let Some(c) = cached {
                    return Ok(c);
                }
            }
            if status.is_success() {
                let body = resp.text()?;
                if !body.trim().is_empty() {
                    let _ = std::fs::write(&cache_file, &body);
                    let _ = std::fs::write(&meta_file, format!("{}\n{}", etag, lm));
                    return Ok(body);
                }
            }
            // 异常状态码：回退缓存
            if let Some(c) = cached {
                return Ok(c);
            }
            Err(anyhow!("配置下载失败 code={} 且无本地缓存", status))
        }
        Err(e) => {
            if let Some(c) = cached {
                return Ok(c);
            }
            Err(anyhow!("配置下载失败: {}", e))
        }
    }
}

/// 获取配置原始 JSON 文本
pub fn fetch_config(source: &str) -> Result<String> {
    let text = match classify(source) {
        SourceKind::Http(url) => fetch_http_with_cache(&url)?,
        SourceKind::File(p) => {
            let data = std::fs::read_to_string(&p).map_err(|e| anyhow!("读取配置文件失败 {}: {}", p.display(), e))?;
            data
        }
        SourceKind::Raw => source.to_string(),
    };
    // 去掉 UTF-8 BOM
    let text = text.trim_start_matches('\u{feff}').trim().to_string();
    if text.is_empty() {
        return Err(anyhow!("配置数据为空"));
    }
    if !text.starts_with('{') && !text.starts_with('[') {
        return Err(anyhow!("配置内容不是 JSON（首个 URL 返回了非预期内容？）"));
    }
    Ok(text)
}
