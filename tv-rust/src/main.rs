#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! TV-Multiplatform 的 Rust (egui) 桌面客户端。
//! 架构：egui UI ⇄ (HTTP JSON API) ⇄ spider-host.jar (JVM, 加载 Java 爬虫)

mod app;
mod core;
mod screens;
mod theme;

use std::sync::Arc;

use egui::{IconData, ViewportBuilder};

/// 应用图标：编译期内嵌 256×256 PNG（矢量源文件见 `assets/logo.svg`）。
fn load_icon() -> Arc<IconData> {
    const PNG: &[u8] = include_bytes!("../assets/icon.png");
    let icon = image::load_from_memory(PNG)
        .map(|img| {
            let img = img.to_rgba8();
            let (width, height) = img.dimensions();
            IconData {
                rgba: img.into_raw(),
                width,
                height,
            }
        })
        .unwrap_or(IconData {
            rgba: vec![0, 0, 0, 0],
            width: 1,
            height: 1,
        });
    Arc::new(icon)
}

fn main() -> eframe::Result<()> {
    core::log::init();
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([960.0, 620.0])
            .with_title("TV Rust — TV-Multiplatform")
            .with_icon(load_icon()),
        ..Default::default()
    };
    eframe::run_native("tv-rust", options, Box::new(|cc| Ok(Box::new(app::TVApp::new(cc)))))
}
