//! 首页：站源选择 + 分类 + 筛选 + 海报网格 + 分页

use crate::app::TVApp;
use crate::core::models::{Filter, Type, Vod};
use crate::theme::{ACCENT, CARD, TEXT, TEXT_DIM};
use egui::{Align2, Color32, Pos2, Rect, RichText, ScrollArea, Sense, Vec2};

#[derive(Default)]
pub struct HomeState {
    pub site_key: Option<String>,
    pub site_name: String,
    pub types: Vec<Type>,
    pub selected_type: usize,
    /// tid -> 筛选组列表
    pub filters: std::collections::HashMap<String, Vec<Filter>>,
    /// filter.key -> 选中的 value.v
    pub choice: std::collections::HashMap<String, String>,
    pub vods: Vec<Vod>,
    pub page: i32,
    pub pagecount: i32,
    pub loading: bool,
}

impl HomeState {
    pub fn reset(&mut self) {
        self.site_key = None;
        self.site_name.clear();
        self.types.clear();
        self.selected_type = 0;
        self.filters.clear();
        self.choice.clear();
        self.vods.clear();
        self.page = 1;
        self.pagecount = 1;
        self.loading = false;
    }
}

impl TVApp {
    pub fn ui_home(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("home_sites").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("站源").color(TEXT_DIM));
                ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let Some(config) = self.config.clone() else {
                            ui.label(RichText::new("请先在设置页添加并加载配置").color(TEXT_DIM));
                            return;
                        };
                        for site in &config.sites {
                            let selected = self.home.site_key.as_deref() == Some(site.key.as_str());
                            let label = RichText::new(&site.name).color(if selected { ACCENT } else { TEXT });
                            if ui.add(egui::Button::new(label).fill(if selected { CARD } else { crate::theme::BG_PANEL })).clicked() {
                                self.select_site(site.clone());
                            }
                        }
                    });
                });
            });
            ui.add_space(4.0);
        });

        let mut clicked_type: Option<usize> = None;
        egui::TopBottomPanel::top("home_types").show(ctx, |ui| {
            ui.add_space(2.0);
            ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, t) in self.home.types.iter().enumerate() {
                        let selected = self.home.selected_type == i;
                        let label = RichText::new(&t.type_name).color(if selected { ACCENT } else { TEXT_DIM });
                        if ui.add(egui::Button::new(label).fill(Color32::TRANSPARENT)).clicked() {
                            clicked_type = Some(i);
                        }
                    }
                });
            });
            ui.add_space(2.0);
        });
        if let Some(i) = clicked_type {
            self.select_category(i);
        }

        // 筛选行
        let active_tid = self
            .home
            .types
            .get(self.home.selected_type)
            .map(|t| t.type_id.clone())
            .unwrap_or_default();
        let filters = self.home.filters.get(&active_tid).cloned().unwrap_or_default();
        if !filters.is_empty() {
            egui::TopBottomPanel::top("home_filters").show(ctx, |ui| {
                egui::CollapsingHeader::new(RichText::new("筛选").color(TEXT_DIM))
                    .default_open(false)
                    .show(ui, |ui| {
                        for f in &filters {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(f.name.clone().unwrap_or_default()).color(TEXT_DIM).size(12.0));
                                for v in &f.value {
                                    let key = f.key.clone().unwrap_or_default();
                                    let selected = self.home.choice.get(&key).map(|c| c == &v.v.clone().unwrap_or_default()).unwrap_or(false);
                                    let label = RichText::new(v.n.clone().unwrap_or_default()).color(if selected { ACCENT } else { TEXT });
                                    if ui.add(egui::Button::new(label).fill(Color32::TRANSPARENT)).clicked() {
                                        if selected {
                                            self.home.choice.remove(&key);
                                        } else {
                                            self.home.choice.insert(key.clone(), v.v.clone().unwrap_or_default());
                                        }
                                        self.select_category(self.home.selected_type);
                                    }
                                }
                            });
                        }
                    });
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.home.loading && self.home.vods.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Spinner::new().size(32.0));
                    ui.label(RichText::new("加载中…").color(TEXT_DIM));
                });
                return;
            }
            let vods = self.home.vods.clone();
            if vods.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("暂无内容").color(TEXT_DIM));
                });
                return;
            }
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let cell_w = 132.0;
                let gap = 10.0;
                let cols = ((ui.available_width() / (cell_w + gap)).floor() as usize).max(1);
                let site_key = self.home.site_key.clone().unwrap_or_default();
                let chunks: Vec<&[Vod]> = vods.chunks(cols).collect();
                egui::Grid::new("home_grid").spacing([gap as f32, 14.0]).show(ui, |ui| {
                    for chunk in chunks {
                        for vod in chunk {
                            ui.vertical(|ui| {
                                let resp = self.poster_cell(ui, &vod.vod_pic, &vod.vod_name, &vod.vod_remarks, cell_w);
                                if resp.clicked() && !site_key.is_empty() {
                                    self.open_detail(site_key.clone(), vod.vod_id.clone());
                                }
                            });
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(10.0);
                self.pagination_bar(ui);
            });
        });
    }

    fn pagination_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.home.loading {
                ui.add(egui::Spinner::new().size(16.0));
            }
            ui.label(
                RichText::new(format!("第 {} / {} 页", self.home.page.max(1), self.home.pagecount.max(1)))
                    .color(TEXT_DIM),
            );
            let has_more = self.home.page < self.home.pagecount;
            if ui
                .add_enabled(has_more && !self.home.loading, egui::Button::new(RichText::new("加载更多").color(if has_more { ACCENT } else { TEXT_DIM })))
                .clicked()
            {
                self.load_more_category();
            }
        });
    }

    /// 海报单元：图 + 名称 + 备注；返回点击响应
    pub fn poster_cell(
        &mut self,
        ui: &mut egui::Ui,
        pic: &str,
        name: &str,
        remarks: &str,
        w: f32,
    ) -> egui::Response {
        let pic_h = w * 1.42;
        let text_h = 20.0 + 18.0;
        let size = Vec2::new(w, pic_h + text_h);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter_at(rect);

        let pic_rect = Rect::from_min_size(rect.min, Vec2::new(w, pic_h));
        if resp.hovered() {
            painter.rect_filled(pic_rect.expand(2.0), 6.0, ACCENT.gamma_multiply(0.15));
        }

        let tex = if pic.is_empty() { None } else { self.textures.get(pic) };
        match tex {
            Some(h) => {
                painter.image(
                    h.id(),
                    pic_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(pic_rect, 6.0, CARD);
                painter.text(
                    pic_rect.center(),
                    Align2::CENTER_CENTER,
                    "🎬",
                    egui::FontId::proportional(26.0),
                    TEXT_DIM,
                );
            }
        }
        if !remarks.is_empty() {
            let badge = painter.text(
                Pos2::new(pic_rect.right() - 4.0, pic_rect.bottom() - 4.0),
                Align2::RIGHT_BOTTOM,
                remarks,
                egui::FontId::proportional(11.0),
                Color32::from_rgb(255, 200, 90),
            );
            let _ = badge;
        }

        let name_display = truncate_chars(name, 14);
        painter.text(
            Pos2::new(rect.min.x, pic_rect.bottom() + 6.0),
            Align2::LEFT_TOP,
            name_display,
            egui::FontId::proportional(13.0),
            TEXT,
        );
        resp
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{}…", t)
    }
}
