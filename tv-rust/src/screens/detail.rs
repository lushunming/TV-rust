//! 详情页：影片信息 + 线路 + 选集 + 外部播放

use crate::app::TVApp;
use crate::core::models::Episode;
use crate::theme::{ACCENT, CARD, TEXT, TEXT_DIM};
use egui::{Color32, RichText, ScrollArea, Vec2};

#[derive(Default)]
pub struct DetailState {
    pub site_key: String,
    pub vod: Option<crate::core::models::Vod>,
    pub flags: Vec<crate::core::models::Flag>,
    pub flag_idx: usize,
    pub ep_page: usize,
    pub loading: bool,
    pub playing: bool,
    pub error: Option<String>,
}

const EP_PAGE_SIZE: usize = 15;

impl TVApp {
    pub fn ui_detail(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("detail_top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("← 返回").color(ACCENT)).clicked() {
                    self.screen = self.detail_back;
                }
                if let Some(vod) = &self.detail.vod {
                    ui.heading(RichText::new(&vod.vod_name).color(TEXT));
                } else {
                    ui.heading(RichText::new("详情").color(TEXT));
                }
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.detail.loading {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Spinner::new().size(32.0));
                    ui.label(RichText::new("加载详情中…").color(TEXT_DIM));
                });
                return;
            }
            if let Some(err) = &self.detail.error {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(err).color(Color32::from_rgb(240, 120, 120)));
                });
                return;
            }
            let Some(vod) = self.detail.vod.clone() else { return };
            let flags = self.detail.flags.clone();

            let avail = ui.available_size();
            let left_w = 200.0;
            let right_w = (avail.x - left_w - 26.0).max(260.0);
            ui.horizontal_top(|ui| {
                // 左列：海报 + 元信息
                ui.allocate_ui(Vec2::new(left_w, ui.available_height()), |ui| {
                    ui.vertical(|ui| {
                        self.poster_cell_static(ui, &vod.vod_pic, left_w - 10.0);
                        ui.add_space(8.0);
                        meta_row(ui, "类型", &vod.type_name);
                        meta_row(ui, "年份", &vod.vod_year);
                        meta_row(ui, "地区", &vod.vod_area);
                        meta_row(ui, "备注", &vod.vod_remarks);
                    });
                });
                ui.separator();
                // 右列：简介 + 线路 + 选集
                // 注意：horizontal 布局中 Label 默认不换行，必须给定固定宽度，否则长文本会溢出到窗口右边界
                ui.allocate_ui_with_layout(
                    Vec2::new(right_w, avail.y),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        if !vod.vod_director.is_empty() || !vod.vod_actor.is_empty() {
                            ui.label(
                                RichText::new(format!("导演: {}   主演: {}", vod.vod_director, vod.vod_actor))
                                    .color(TEXT_DIM)
                                    .size(12.5),
                            );
                        }
                        if !vod.vod_content.is_empty() {
                            ui.add_space(4.0);
                            ui.label(RichText::new(clean_html(&vod.vod_content)).color(TEXT).size(12.5));
                        }
                        ui.add_space(8.0);

                        if flags.is_empty() {
                            ui.label(RichText::new("无可用播放线路").color(TEXT_DIM));
                            return;
                        }
                        // 线路 tabs
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("线路:").color(TEXT_DIM));
                            for (i, f) in flags.iter().enumerate() {
                                let selected = self.detail.flag_idx == i;
                                let label = RichText::new(&f.name).color(if selected { ACCENT } else { TEXT });
                                if ui.add(egui::Button::new(label).fill(if selected { CARD } else { Color32::TRANSPARENT })).clicked() {
                                    self.detail.flag_idx = i;
                                    self.detail.ep_page = 0;
                                }
                            }
                        });
                        ui.add_space(6.0);

                        let Some(flag) = flags.get(self.detail.flag_idx) else { return };
                        let total = flag.episodes.len();
                        let pages = total.div_ceil(EP_PAGE_SIZE);
                        let start = self.detail.ep_page.min(pages.saturating_sub(1)) * EP_PAGE_SIZE;
                        let end = (start + EP_PAGE_SIZE).min(total);

                        // 分页
                        ui.horizontal(|ui| {
                            if pages > 1 {
                                if ui.add_enabled(self.detail.ep_page > 0, egui::Button::new("←")).clicked() {
                                    self.detail.ep_page -= 1;
                                }
                                ui.label(RichText::new(format!("共 {} 集 · 第 {}/{} 页", total, self.detail.ep_page + 1, pages)).color(TEXT_DIM));
                                if ui.add_enabled(self.detail.ep_page + 1 < pages, egui::Button::new("→")).clicked() {
                                    self.detail.ep_page += 1;
                                }
                            } else {
                                ui.label(RichText::new(format!("共 {} 集", total)).color(TEXT_DIM));
                            }
                        });
                        ui.add_space(4.0);

                        let eps: Vec<Episode> = flag.episodes[start..end].to_vec();
                        let cols = ((ui.available_width() / (86.0 + 8.0)).floor() as usize).max(1);
                        let chunks: Vec<&[Episode]> = eps.chunks(cols).collect();
                        egui::Grid::new("ep_grid").spacing([8.0, 6.0]).show(ui, |ui| {
                            for chunk in chunks {
                                for ep in chunk {
                                    let label = RichText::new(trunc(&ep.name, 9)).color(TEXT);
                                    if ui.add(egui::Button::new(label).fill(CARD).min_size(Vec2::new(84.0, 28.0))).clicked() {
                                        self.play_episode(&flag.name, ep.clone());
                                    }
                                }
                                ui.end_row();
                            }
                        });

                        if self.detail.playing {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.label(RichText::new("正在获取播放地址…").color(TEXT_DIM));
                            });
                        }
                    });
                });
            });
        });
    }

    /// 不带名称的海报（详情页左列用）
    fn poster_cell_static(&mut self, ui: &mut egui::Ui, pic: &str, w: f32) {
        let h = w * 1.42;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let site_key = self.detail.site_key.clone();
        let tex = if pic.is_empty() { None } else { self.poster_texture(pic, &site_key) };
        match tex {
            Some(t) => {
                painter.image(
                    t.id(),
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(rect, 6.0, CARD);
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, "🎬", egui::FontId::proportional(28.0), TEXT_DIM);
            }
        };
    }
}

fn meta_row(ui: &mut egui::Ui, k: &str, v: &str) {
    if !v.is_empty() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}:", k)).color(TEXT_DIM).size(12.0));
            ui.label(RichText::new(trunc(v, 16)).color(TEXT).size(12.0));
        });
    }
}

fn clean_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{}…", t)
    }
}
