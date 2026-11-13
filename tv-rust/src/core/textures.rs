//! 海报纹理缓存：后台线程下载 + 解码，UI 线程生成 TextureHandle。

use anyhow::Result;
use egui::{Color32, ColorImage, TextureHandle, TextureOptions};
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

    /// 取纹理；未缓存时自动后台拉取。返回 Some(handle) 表示可用。
    pub fn get(&mut self, url: &str) -> Option<TextureHandle> {
        if url.is_empty() {
            return None;
        }
        match self.map.get(url) {
            Some(TexState::Ready(h)) => Some(h.clone()),
            Some(_) => None,
            None => {
                if self.inflight < MAX_INFLIGHT {
                    self.map.insert(url.to_string(), TexState::Pending);
                    self.inflight += 1;
                    let tx = self.tx.clone();
                    let url = url.to_string();
                    std::thread::Builder::new()
                        .name("tex-download".into())
                        .spawn(move || {
                            let res = download_bytes(&url);
                            let _ = tx.send((url, res));
                        })
                        .ok();
                }
                None
            }
        }
    }
}

fn download_bytes(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(crate::core::config::CHROME_UA)
        .build()?;
    let bytes = client.get(url).send()?.error_for_status()?.bytes()?;
    Ok(bytes.to_vec())
}

fn decode_rgba(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w as usize, h as usize, rgba.into_raw()))
}
