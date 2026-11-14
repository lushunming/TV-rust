//! 海报纹理缓存：后台线程下载 + 解码，UI 线程生成 TextureHandle。
//! 下载时可携带站点 header（很多站点对图片做了 Referer/UA 防盗链）；
//! 缓存键同时包含 header，避免不同站点的同名图片互相污染。

use anyhow::Result;
use egui::{ColorImage, TextureHandle, TextureOptions};
use md5::{Digest, Md5};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

enum TexState {
    Pending,
    Ready(TextureHandle),
    Failed,
}

const MAX_CACHED: usize = 600;
const MAX_INFLIGHT: usize = 12;

pub struct TextureCache {
    map: HashMap<String, TexState>,
    order: VecDeque<String>,
    rx: Receiver<(String, Result<Vec<u8>>)>,
    tx: Sender<(String, Result<Vec<u8>>)>,
    inflight: usize,
}

impl Default for TextureCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TextureCache {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        TextureCache { map: HashMap::new(), order: VecDeque::new(), rx, tx, inflight: 0 }
    }

    /// 每帧调用：收割下载结果并生成纹理
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut results = Vec::new();
        while let Ok((url, res)) = self.rx.try_recv() {
            self.inflight = self.inflight.saturating_sub(1);
            results.push((url, res));
        }
        for (url, res) in results {
            match res {
                Ok(bytes) => match decode_rgba(&bytes) {
                    Some((w, h, rgba)) => {
                        let img = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                        let handle = ctx.load_texture(&format!("tex:{}", url), img, TextureOptions::default());
                        self.map.insert(url.clone(), TexState::Ready(handle));
                        self.order.push_back(url);
                    }
                    None => {
                        self.map.insert(url.clone(), TexState::Failed);
                    }
                },
                Err(_) => {
                    self.map.insert(url.clone(), TexState::Failed);
                }
            }
        }
        self.prune();
    }

    fn prune(&mut self) {
        while self.order.len() > MAX_CACHED {
            // 先清最旧的条目（Ready/Pending 一并淘汰，Pending 淘汰后下次会重新拉取）
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            } else {
                break;
            }
        }
        if self.map.len() > MAX_CACHED * 2 {
            let failed: Vec<String> = self
                .map
                .iter()
                .filter(|(_, s)| matches!(s, TexState::Failed))
                .map(|(k, _)| k.clone())
                .collect();
            for k in failed {
                self.map.remove(&k);
            }
        }
    }

    /// 取纹理（不带额外 header）；未缓存时自动后台拉取。
    #[allow(dead_code)]
    pub fn get(&mut self, url: &str) -> Option<TextureHandle> {
        self.get_with_headers(url, &[])
    }

    /// 取纹理，携带站点 header。返回 Some 表示纹理已可用。
    pub fn get_with_headers(&mut self, url: &str, headers: &[(String, String)]) -> Option<TextureHandle> {
        if url.is_empty() {
            return None;
        }
        if headers.is_empty() {
            return self.get_cached(url);
        }
        let key = cache_key(url, headers);
        self.get_cached(&key)
    }

    fn get_cached(&mut self, key: &str) -> Option<TextureHandle> {
        match self.map.get(key) {
            Some(TexState::Ready(h)) => Some(h.clone()),
            Some(_) => None,
            None => {
                if self.inflight < MAX_INFLIGHT {
                    self.map.insert(key.to_string(), TexState::Pending);
                    self.inflight += 1;
                    let tx = self.tx.clone();
                    let key_owned = key.to_string();
                    std::thread::Builder::new()
                        .name("tex-download".into())
                        .spawn(move || {
                            let res = download_bytes(&key_owned);
                            let _ = tx.send((key_owned, res));
                        })
                        .ok();
                }
                None
            }
        }
    }
}

/// 缓存键：URL 本身，或 "URL#<header 摘要>"。
/// 下载线程只需键，因此把 header 以 `url\nk: v\n...` 的形式编码进键的前缀部分不可行，
/// 改为：键 = url + "#" + 8 位 md5，下载时由 `split_key` 还原 URL 与 header。
fn cache_key(url: &str, headers: &[(String, String)]) -> String {
    if headers.is_empty() {
        return url.to_string();
    }
    let mut h = Md5::new();
    for (k, v) in headers {
        h.update(k.as_bytes());
        h.update(b":");
        h.update(v.as_bytes());
        h.update(b"\n");
    }
    let digest: String = h.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    let mut key = String::with_capacity(url.len() + 10);
    key.push_str(url);
    key.push('#');
    key.push_str(&digest[..8]);
    // 把 header 一并编码，供下载线程还原（分隔符用不可见控制字符，避免与 URL 冲突）
    for (k, v) in headers {
        key.push('\u{1}');
        key.push_str(k);
        key.push('\u{2}');
        key.push_str(v);
    }
    key
}

fn split_key(key: &str) -> (String, Vec<(String, String)>) {
    let mut parts = key.split('\u{1}');
    let head = parts.next().unwrap_or("");
    let url = head.split('#').next().unwrap_or(head).to_string();
    let headers = parts
        .filter_map(|seg| {
            let mut kv = seg.splitn(2, '\u{2}');
            let k = kv.next()?.to_string();
            let v = kv.next()?.to_string();
            Some((k, v))
        })
        .collect();
    (url, headers)
}

fn download_bytes(key: &str) -> Result<Vec<u8>> {
    let (url, headers) = split_key(key);
    let mut req = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(crate::core::config::CHROME_UA);
    if !headers.is_empty() {
        let mut map = reqwest::header::HeaderMap::new();
        for (k, v) in &headers {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                map.insert(name, value);
            }
        }
        req = req.default_headers(map);
    }
    let client = req.build()?;
    let bytes = client.get(&url).send()?.error_for_status()?.bytes()?;
    Ok(bytes.to_vec())
}

fn decode_rgba(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w as usize, h as usize, rgba.into_raw()))
}
