//! 历史记录页

use crate::app::TVApp;
use crate::theme::{TEXT, TEXT_DIM};
use egui::{RichText, ScrollArea};

impl TVApp {
    pub fn ui_history(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("history_top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("观看历史").color(crate::theme::TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(RichText::new("清空全部").color(crate::theme::DANGER)).clicked() {
                        self.history_store.clear();
                        self.toast("历史已清空");
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let entries = self.history_store.list();
            if entries.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("暂无观看记录").color(TEXT_DIM));
                });
                return;
            }
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for e in entries {
                    ui.horizontal(|ui| {
                        let label = RichText::new(format!("{} · {}", e.vod_name, e.ep_name)).color(TEXT);
                        if ui.add(egui::Button::new(label).fill(crate::theme::CARD)).clicked() {
                            let (sk, vid) = (e.site_key.clone(), e.vod_id.clone());
                            self.open_detail(sk, vid);
                        }
                        ui.label(RichText::new(trunc_time(e.create_time)).color(TEXT_DIM).size(11.5));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(RichText::new("删除").size(12.0).color(TEXT_DIM)).clicked() {
                                self.history_store.remove(&e.key);
                            }
                        });
                    });
                    ui.separator();
                }
            });
        });
    }
}

fn trunc_time(ms: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let d = UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64);
    let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as i64 - ms / 1000;
    let human = if let Ok(elapsed) = d.elapsed() {
        let mins = elapsed.as_secs() / 60;
        if mins < 1 {
            "刚刚".to_string()
        } else if mins < 60 {
            format!("{} 分钟前", mins)
        } else if mins < 60 * 24 {
            format!("{} 小时前", mins / 60)
        } else {
            format!("{} 天前", mins / 60 / 24)
        }
    } else {
        String::new()
    };
    let _ = secs;
    human
}
