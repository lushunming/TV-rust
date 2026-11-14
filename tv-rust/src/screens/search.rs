//! 搜索页：输入 + 分批搜索 + 结果流式追加

use crate::app::TVApp;
use crate::core::models::Vod;
use crate::theme::{ACCENT, TEXT_DIM};
use egui::{RichText, ScrollArea};

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub site_key: String,
    pub site_name: String,
    pub vod: Vod,
}

#[derive(Default)]
pub struct SearchState {
    pub input: String,
    pub query: String,
    pub results: Vec<SearchHit>,
    pub sites: Vec<crate::core::models::Site>,
    pub next_batch: usize,
    pub pending: usize,
    pub error_count: usize,
}

impl SearchState {
    pub fn reset(&mut self, query: String) {
        self.query = query;
        self.results.clear();
        self.sites.clear();
        self.next_batch = 0;
        self.pending = 0;
        self.error_count = 0;
    }
}

impl TVApp {
    pub fn ui_search(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("search_bar").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search.input)
                        .hint_text("输入关键词搜索（回车开始）")
                        .desired_width(ui.available_width() - 110.0),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button(RichText::new("搜索").color(ACCENT)).clicked() || enter {
                    self.do_search();
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if !self.search.query.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "关键词: {}  ·  结果 {} 条  ·  {} / {} 站点完成",
                            self.search.query,
                            self.search.results.len(),
                            self.search.next_batch - self.search.pending,
                            self.search.sites.len()
                        ))
                        .color(TEXT_DIM),
                    );
                }
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let hits = self.search.results.clone();
            if hits.is_empty() && self.search.query.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("输入关键词开始搜索").color(TEXT_DIM));
                });
                return;
            }
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let cell_w = 150.0;
                let gap = 10.0;
                let cols = ((ui.available_width() / (cell_w + gap)).floor() as usize).max(1);
                let chunks: Vec<&[SearchHit]> = hits.chunks(cols).collect();
                egui::Grid::new("search_grid").spacing([gap as f32, 14.0]).show(ui, |ui| {
                    for chunk in chunks {
                        for hit in chunk {
                            ui.vertical(|ui| {
                                let resp = self.poster_cell(ui, &hit.vod.vod_pic, &hit.vod.vod_name, &hit.vod.vod_remarks, cell_w, &hit.site_key);
                                if resp.clicked() {
                                    let (sk, vid) = (hit.site_key.clone(), hit.vod.vod_id.clone());
                                    self.open_detail(sk, vid);
                                }
                                ui.label(RichText::new(trunc(&hit.site_name, 10)).color(TEXT_DIM).size(11.0));
                            });
                        }
                        ui.end_row();
                    }
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if self.search.pending > 0 {
                        ui.add(egui::Spinner::new().size(16.0));
                        ui.label(RichText::new("搜索中…").color(TEXT_DIM));
                    }
                    let has_more = self.search.next_batch < self.search.sites.len();
                    if ui
                        .add_enabled(has_more && self.search.pending == 0, egui::Button::new(RichText::new("加载更多站点").color(if has_more { ACCENT } else { TEXT_DIM })))
                        .clicked()
                    {
                        self.search_more();
                    }
                });
                ui.add_space(10.0);
            });
        });
    }
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{}…", t)
    }
}
