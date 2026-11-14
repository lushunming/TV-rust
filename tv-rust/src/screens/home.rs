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
    /// 正在追加下一页（滚动加载）
    pub loading_more: bool,
    /// 已经到底（没有下一页或下一页为空）
    pub no_more: bool,
    /// 站源下拉框内部的搜索词（不持久化）
    pub site_filter: String,
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
        self.loading_more = false;
        self.no_more = false;
        self.site_filter.clear();
    }
}

impl TVApp {
    pub fn ui_home(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("home_sites").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let Some(config) = self.config.clone() else {
                    ui.label(RichText::new("请先在设置页添加并加载配置").color(TEXT_DIM));
                    ui.add_space(6.0);
                    return;
                };

                ui.label(RichText::new("站源").color(TEXT_DIM).size(13.0));

                let current = self
                    .home
                    .site_key
                    .as_deref()
                    .and_then(|k| config.sites.iter().find(|s| s.key == k))
                    .map(|s| s.name.clone())
                    .unwrap_or_default();

                let mut picked: Option<crate::core::models::Site> = None;
                let combo_text = if current.is_empty() {
                    RichText::new("选择站源").color(TEXT_DIM)
                } else {
                    RichText::new(current.clone()).color(ACCENT).strong()
                };

                egui::ComboBox::from_id_salt("site_combo")
                    .width(240.0)
                    .selected_text(combo_text)
                    .show_ui(ui, |ui| {
                        ui.add_space(4.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.home.site_filter)
                                .hint_text("搜索站点名称 / key / 类型")
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(6.0);
                        ui.separator();

                        let needle = self.home.site_filter.trim().to_lowercase();
                        let mut shown = 0usize;
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for site in &config.sites {
                                    if !needle.is_empty() {
                                        let hay = format!("{} {} {}", site.name, site.key, site.api).to_lowercase();
                                        if !hay.contains(&needle) {
                                            continue;
                                        }
                                    }
                                    shown += 1;
                                    let selected = self.home.site_key.as_deref() == Some(site.key.as_str());
                                    let text = if selected {
                                        RichText::new(&site.name).color(ACCENT).strong()
                                    } else {
                                        RichText::new(&site.name).color(TEXT)
                                    };
                                    let resp = ui.add_sized(
                                        [ui.available_width(), 28.0],
                                        egui::Button::selectable(selected, text).frame_when_inactive(false),
                                    );
                                    if resp.clicked() {
                                        picked = Some(site.clone());
                                        ui.close();
                                    }
                                }
                                if shown == 0 {
                                    ui.add_space(4.0);
                                    ui.label(RichText::new("没有匹配的站点").color(TEXT_DIM).size(12.0));
                                }
                            });
                    });

                if let Some(site) = picked {
                    self.home.site_filter.clear();
                    self.select_site(site);
                }

                ui.add_space(10.0);
                if let Some(site) = config.sites.iter().find(|s| Some(s.key.as_str()) == self.home.site_key.as_deref()) {
                    let kind = site.api.trim_start_matches("csp_").to_string();
                    if !kind.is_empty() {
                        ui.label(RichText::new(kind).color(TEXT_DIM).size(12.0));
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} 个站点", config.sites.len()))
                            .color(TEXT_DIM)
                            .size(12.0),
                    );
                });
            });
            ui.add_space(6.0);
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
            let mut want_more = false;
            ScrollArea::vertical().auto_shrink(false).show_viewport(ui, |ui, viewport| {
                let cell_w = 150.0;
                let gap = 10.0;
                let cols = ((ui.available_width() / (cell_w + gap)).floor() as usize).max(1);
                let site_key = self.home.site_key.clone().unwrap_or_default();
                let chunks: Vec<&[Vod]> = vods.chunks(cols).collect();
                egui::Grid::new("home_grid").spacing([gap as f32, 14.0]).show(ui, |ui| {
                    for chunk in chunks {
                        for vod in chunk {
                            ui.vertical(|ui| {
                                let resp = self.poster_cell(ui, &vod.vod_pic, &vod.vod_name, &vod.vod_remarks, cell_w, &site_key);
                                if resp.clicked() && !site_key.is_empty() {
                                    self.open_detail(site_key.clone(), vod.vod_id.clone());
                                }
                            });
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(14.0);
                // 底部状态 + 滚动加载
                if self.home.selected_type > 0 {
                    let total = self.home.vods.len();
                    ui.vertical_centered(|ui| {
                        if self.home.loading_more {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.label(RichText::new("加载中…").color(TEXT_DIM));
                            });
                        } else if self.home.no_more || self.home.page >= self.home.pagecount {
                            ui.label(RichText::new(format!("已全部加载 · 共 {total} 部")).color(TEXT_DIM).size(13.0));
                        } else {
                            ui.label(RichText::new("继续向下滚动加载更多").color(TEXT_DIM).size(13.0));
                        }
                    });
                    ui.add_space(12.0);
                }

                let has_more = !self.home.no_more && self.home.page < self.home.pagecount;
                if has_more && !self.home.loading && !self.home.loading_more {
                    let content_h = ui.min_rect().height();
                    let near_bottom = viewport.max.y + 280.0 >= content_h;
                    let scrolled = viewport.min.y > 0.5;
                    let short_content = content_h <= viewport.height() + 1.0;
                    if near_bottom && (scrolled || short_content) {
                        want_more = true;
                    }
                }
            });

            if want_more {
                self.load_more_category();
            }
        });
    }

    /// 海报单元：图 + 名称 + 备注；返回点击响应。site_key 用于取站点 header 拉图。
    pub fn poster_cell(
        &mut self,
        ui: &mut egui::Ui,
        pic: &str,
        name: &str,
        remarks: &str,
        w: f32,
        site_key: &str,
    ) -> egui::Response {
        let pic_h = w * 1.42;
        let text_h = 24.0 + 20.0;
        let size = Vec2::new(w, pic_h + text_h);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter_at(rect);

        let pic_rect = Rect::from_min_size(rect.min, Vec2::new(w, pic_h));
        if resp.hovered() {
            painter.rect_filled(pic_rect.expand(2.0), 6.0, ACCENT.gamma_multiply(0.15));
        }

        let tex = if pic.is_empty() { None } else { self.poster_texture(pic, site_key) };
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
                egui::FontId::proportional(12.0),
                Color32::from_rgb(255, 200, 90),
            );
            let _ = badge;
        }

        let name_display = truncate_chars(name, 9);
        painter.text(
            Pos2::new(rect.min.x, pic_rect.bottom() + 6.0),
            Align2::LEFT_TOP,
            name_display,
            egui::FontId::proportional(15.0),
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
