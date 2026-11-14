#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! TV-Multiplatform 的 Rust (egui) 桌面客户端。
//! 架构：egui UI ⇄ (stdio JSON-RPC) ⇄ spider-host.jar (JVM, 加载 Java 爬虫)

mod app;
mod core;
mod screens;
mod theme;

use egui::ViewportBuilder;

fn main() -> eframe::Result<()> {
    core::log::init();
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([960.0, 620.0])
            .with_title("TV Rust — TV-Multiplatform"),
        ..Default::default()
    };
    eframe::run_native("tv-rust", options, Box::new(|cc| Ok(Box::new(app::TVApp::new(cc)))))
}
