//! 搜索页：输入 + 分批搜索 + 结果流式追加

use crate::app::{AppEvent, TVApp};
use crate::core::hot::HotData;
use crate::core::models::Vod;
use crate::theme::{ACCENT, CARD, TEXT, TEXT_DIM};
use egui::{Align2, FontId, Pos2, RichText, ScrollArea, Sense, Vec2};
use std::time::{Duration, Instant};

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
    pub suggest: SuggestState,
}

/// 搜索联想状态（对齐原 `SearchBar.kt`：500ms 防抖 + 丢弃过期响应）
#[derive(Default)]
pub struct SuggestState {
    /// 当前展示的联想词
    pub items: Vec<String>,
    /// items 对应的关键词
    pub for_query: String,
    /// 正在请求中的关键词（避免同一词重复发请求）
    pub pending_for: String,
    /// 最近一次输入变化时间，用于 500ms 防抖
    pub dirty_at: Option<Instant>,
    /// 搜索已执行/已按 Esc：在下次输入前不再弹出
    pub suppressed: bool,
}

impl SuggestState {
    pub fn on_input(&mut self) {
        self.dirty_at = Some(Instant::now());
        self.suppressed = false;
    }

    pub fn close(&mut self) {
        self.items.clear();
        self.for_query.clear();
        self.pending_for.clear();
        self.dirty_at = None;
    }

    /// 收起且暂停弹出，直到用户再次输入（对齐原 `searchFun` 里清空 suggestions 的行为）
    pub fn suppress(&mut self) {
        self.close();
        self.suppressed = true;
    }

    /// 只接受与在途请求关键词一致的响应，否则丢弃
    pub fn absorb(&mut self, query: String, items: Vec<String>) {
        if query == self.pending_for {
            self.items = items;
            self.for_query = query;
            self.pending_for.clear();
        }
    }
}

impl SearchState {
    pub fn reset(&mut self, query: String) {
        self.query = query;
        self.results.clear();
        self.sites.clear();
        self.next_batch = 0;
        self.pending = 0;
        self.error_count = 0;
        self.suggest.suppress();
    }
}

impl TVApp {
    pub fn ui_search(&mut self, ctx: &egui::Context) {
        let mut input_rect: Option<egui::Rect> = None;
        egui::TopBottomPanel::top("search_bar").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search.input)
                        .hint_text("输入关键词搜索（回车开始）")
                        .desired_width(ui.available_width() - 110.0),
                );
                if resp.changed() {
                    self.search.suggest.on_input();
                }
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button(RichText::new("搜索").color(ACCENT)).clicked() || enter {
                    self.search.suggest.suppress();
                    self.do_search();
                }
                input_rect = Some(resp.rect);
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

        // 联想下拉层（画在面板之后，避免被面板裁切）
        if let Some(rect) = input_rect {
            self.pump_suggest(ctx, rect);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let hits = self.search.results.clone();
            if hits.is_empty() && self.search.query.is_empty() {
                // 搜索页空态即热搜面板（对齐原项目 SearchPage 的 HotPanel）
                if let Some(title) = hot_panel(ui, &self.hot.items) {
                    self.search.input = title;
                    self.do_search();
                }
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

/// 热搜面板：对齐原项目 `HotPanel` / `HotItem`
/// title 卡片主文字为热搜标题，副文字为 `upinfo`；悬停显示 `comment` + `description`；点击返回词条用于直接搜索
fn hot_panel(ui: &mut egui::Ui, items: &[HotData]) -> Option<String> {
    let mut picked: Option<String> = None;

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("热搜").color(TEXT).size(20.0).strong());
        ui.add_space(8.0);
        if !items.is_empty() {
            ui.label(RichText::new(format!("{} 条", items.len())).color(TEXT_DIM).size(12.0));
        }
    });
    ui.add_space(8.0);

    if items.is_empty() {
        ui.label(RichText::new("热搜暂不可用：需要联网拉取，后台刷新完成后会自动出现").color(TEXT_DIM));
        return None;
    }

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for h in items {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(230.0, 66.0), Sense::click());
                    let painter = ui.painter_at(rect);

                    if resp.hovered() {
                        painter.rect_filled(rect.expand(1.0), 13.0, ACCENT.gamma_multiply(0.45));
                    }
                    painter.rect_filled(rect, 12.0, CARD);

                    painter.text(
                        Pos2::new(rect.left() + 12.0, rect.top() + 11.0),
                        Align2::LEFT_TOP,
                        trunc(h.title.trim(), 12),
                        FontId::proportional(15.0),
                        TEXT,
                    );
                    let sub = if h.upinfo.trim().is_empty() {
                        "—".to_string()
                    } else {
                        trunc(h.upinfo.trim(), 14)
                    };
                    painter.text(
                        Pos2::new(rect.left() + 12.0, rect.top() + 36.0),
                        Align2::LEFT_TOP,
                        sub,
                        FontId::proportional(11.5),
                        TEXT_DIM,
                    );

                    let has_tip = !h.comment.trim().is_empty() || !h.description.trim().is_empty();
                    let resp = if has_tip {
                        resp.on_hover_ui(|ui| {
                            ui.set_max_width(300.0);
                            if !h.comment.trim().is_empty() {
                                ui.label(RichText::new(h.comment.trim()).color(TEXT).strong());
                            }
                            if !h.description.trim().is_empty() {
                                ui.add_space(2.0);
                                ui.label(RichText::new(h.description.trim()).color(TEXT_DIM).size(12.0));
                            }
                        })
                    } else {
                        resp
                    };
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        picked = Some(h.title.clone());
                    }
                }
            });
        });

    picked
}

impl TVApp {
    /// 搜索联想：输入停顿 500ms 后请求 iqiyi suggest，结果用浮层列出（对齐原 `SearchBar.kt`）
    fn pump_suggest(&mut self, ctx: &egui::Context, anchor: egui::Rect) {
        let q = self.search.input.trim().to_string();
        if q.is_empty() {
            self.search.suggest.close();
            return;
        }
        // 搜索已执行或已按 Esc：等用户再次输入才恢复
        if self.search.suggest.suppressed {
            return;
        }

        // 防抖：输入变化后固定等待 500ms，期间不请求
        if let Some(t) = self.search.suggest.dirty_at {
            let due = t + Duration::from_millis(500);
            let now = Instant::now();
            if now < due {
                ctx.request_repaint_after(due - now);
                return;
            }
            self.search.suggest.dirty_at = None;
        }

        if q != self.search.suggest.for_query && q != self.search.suggest.pending_for {
            self.search.suggest.pending_for = q.clone();
            let tx = self.event_tx.clone();
            let key = q.clone();
            std::thread::spawn(move || match crate::core::suggest::fetch(&key) {
                Ok(items) => {
                    let _ = tx.send(AppEvent::Suggest { query: key, items });
                }
                Err(e) => {
                    crate::core::log::warn(format!("联想请求失败: {e:#}"));
                    let _ = tx.send(AppEvent::Suggest {
                        query: key,
                        items: Vec::new(),
                    });
                }
            });
        }

        // Esc 收起（下次输入仍可再次弹出）
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search.suggest.suppress();
            return;
        }

        if self.search.suggest.for_query != q || self.search.suggest.items.is_empty() {
            return;
        }

        let items = self.search.suggest.items.clone();
        let mut picked: Option<String> = None;
        egui::Area::new(egui::Id::new("search_suggest"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(crate::theme::BG_PANEL)
                    .stroke(egui::Stroke::new(1.0_f32, CARD))
                    .show(ui, |ui| {
                        ui.set_width(anchor.width());
                        ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            for it in &items {
                                let resp = ui.add_sized(
                                    [ui.available_width(), 26.0],
                                    egui::Button::new(RichText::new(it).color(TEXT)).frame(false),
                                );
                                if resp.clicked() {
                                    picked = Some(it.clone());
                                }
                            }
                        });
                    });
            });

        if let Some(v) = picked {
            self.search.input = v;
            self.search.suggest.suppress();
            self.do_search();
        }
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
