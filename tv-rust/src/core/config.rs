//! 配置源获取：http（带 ETag/Last-Modified 磁盘缓存，与 Kotlin 共用缓存目录）、file、内联 JSON。
//! 对应 Kotlin ApiConfig.getData / fetchConfigWithCache。

use crate::core::models::RawApiConfig;
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

pub fn md5_bytes(bytes: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(bytes);
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

// ---------- 爬虫 jar 预取 ----------
//
// jar 由宿主 JVM 下载时，部分站点的 TLS 指纹会被服务端拒绝（handshake_failure），
// 导致所有 csp_ 站点拿到空列表。Rust 侧 TLS 栈通常能正常握手，
// 因此由前端先把 jar 写入与宿主一致的缓存路径（md5(url).jar），保护性能与可用性。

#[derive(Debug, Default)]
pub struct JarPrefetch {
    pub downloaded: usize,
    pub cached: usize,
    pub failed: Vec<String>,
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    let t = value.trim();
    if !t.is_empty() && !list.iter().any(|x| x == t) {
        list.push(t.to_string());
    }
}

fn is_zip(bytes: &[u8]) -> bool {
    bytes.len() > 4 && bytes[0] == 0x50 && bytes[1] == 0x4B
}

fn download_jar(url: &str, expect_md5: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(CHROME_UA)
        .build()?;
    let bytes = client.get(url).send()?.error_for_status()?.bytes()?.to_vec();
    if !is_zip(&bytes) {
        return Err(anyhow!("下载内容不是 jar/zip (size={})", bytes.len()));
    }
    if !expect_md5.is_empty() {
        let got = md5_bytes(&bytes);
        if !got.eq_ignore_ascii_case(expect_md5) {
            return Err(anyhow!("jar md5 不匹配: 期望 {} 实际 {}", expect_md5, got));
        }
    }
    Ok(bytes)
}

/// 预取配置引用到的全部远程 jar。`config_url` 用于解析相对路径。
pub fn prefetch_spider_jars(config_url: &str, cfg: &RawApiConfig) -> JarPrefetch {
    let mut report = JarPrefetch::default();
    let mut specs: Vec<String> = Vec::new();
    push_unique(&mut specs, &cfg.spider);
    for site in &cfg.sites {
        if let Some(jar) = &site.jar {
            push_unique(&mut specs, jar);
        }
    }
    if specs.is_empty() {
        return report;
    }

    let dir = paths::ensure_dir(&paths::jar_cache());
    for spec in specs {
        let (jar_ref, expect) = match spec.split_once(";md5;") {
            Some((a, b)) => (a.trim(), b.trim().to_string()),
            None => (spec.trim(), String::new()),
        };
        if jar_ref.is_empty() || jar_ref.starts_with("file") {
            continue;
        }
        let url = if jar_ref.starts_with("http://") || jar_ref.starts_with("https://") {
            jar_ref.to_string()
        } else {
            match reqwest::Url::parse(config_url).and_then(|b| b.join(jar_ref)) {
                Ok(u) => u.to_string(),
                Err(e) => {
                    report.failed.push(format!("{}: 无法解析相对路径 ({})", jar_ref, e));
                    continue;
                }
            }
        };

        let path = dir.join(format!("{}.jar", md5_hex(&url)));
        if let Ok(existing) = std::fs::read(&path) {
            if is_zip(&existing) && (expect.is_empty() || md5_bytes(&existing).eq_ignore_ascii_case(&expect)) {
                report.cached += 1;
                continue;
            }
        }

        match download_jar(&url, &expect) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&path, &bytes) {
                    report.failed.push(format!("{}: 写入缓存失败 ({})", url, e));
                } else {
                    eprintln!("[jar] prefetched {} ({} bytes) -> {}", url, bytes.len(), path.display());
                    report.downloaded += 1;
                }
            }
            Err(e) => {
                report.failed.push(format!("{}: {}", url, e));
            }
        }
    }
    report
}
