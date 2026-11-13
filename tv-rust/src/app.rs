//! TVApp：导航、事件分发、spider-host 调度

use crate::core::history::{HistoryEntry, HistoryStore};
use crate::core::host::SpiderHost;
use crate::core::models::{RawApiConfig, SpiderResult, Type, Vod};
use crate::core::settings::Settings;
use crate::core::textures::TextureCache;
use crate::screens::detail::DetailState;
use crate::screens::home::HomeState;
use crate::screens::search::{SearchHit, SearchState};
use crate::screens::settings::SettingsPageState;
use crate::theme;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Search,
    Detail,
    History,
    Settings,
}

#[derive(Debug, Clone)]
pub enum HostStatus {
    Starting,
    Ready(String),
    Failed(String),
}

pub enum AppEvent {
    ConfigReady(RawApiConfig, String),
    ConfigError(String),
    HomeTypes(SpiderResult),
    HomeVideos(SpiderResult),
    CategoryPage { page: i32, result: SpiderResult },
    SearchSiteResult { site_key: String, site_name: String, vods: Vec<Vod>, error: Option<String> },
    DetailLoaded { result: SpiderResult },
    DetailError(String),
    PlayReady {
        url: String,
        headers: HashMap<String, String>,
        site_key: String,
        vod_id: String,
        vod_name: String,
        vod_pic: String,
        flag: String,
        ep_url: String,
        ep_name: String,
    },
    PlayError(String),
    Toast(String),
}

pub struct TVApp {
    pub settings: Settings,
    pub host: Option<Arc<SpiderHost>>,
    pub host_status: HostStatus,
    pub config: Option<RawApiConfig>,
    pub cfg_id: String,

    pub event_tx: Sender<AppEvent>,
    pub event_rx: Receiver<AppEvent>,

    pub screen: Screen,
    pub detail_back: Screen,

    pub home: HomeState,
    pub search: SearchState,
    pub detail: DetailState,
    pub history_store: HistoryStore,
    pub settings_page: SettingsPageState,

    pub textures: TextureCache,
    pub toast: Option<(String, Instant)>,
}

impl TVApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::setup(&cc.egui_ctx);
        let settings = Settings::load();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = TVApp {
            settings_page: SettingsPageState {
                player_path_edit: settings.player_path.clone(),
                java_path_edit: settings.java_path.clone(),
                host_jar_edit: settings.host_jar.clone().unwrap_or_default(),
                ..Default::default()
            },
            settings,
            host: None,
            host_status: HostStatus::Starting,
            config: None,
            cfg_id: String::new(),
            event_tx: tx,
            event_rx: rx,
            screen: Screen::Home,
            detail_back: Screen::Home,
            home: HomeState::default(),
            search: SearchState::default(),
            detail: DetailState::default(),
            history_store: HistoryStore::load(),
            textures: TextureCache::new(),
            toast: None,
        };
        // settings_page 已在上面初始化，重新同步编辑框
        app.settings_page.player_path_edit = app.settings.player_path.clone();
        app.settings_page.java_path_edit = app.settings.java_path.clone();
        app.settings_page.host_jar_edit = app.settings.host_jar.clone().unwrap_or_default();
        app.start_host();
        app
    }

    // ---------- 宿主管理 ----------

    pub fn start_host(&mut self) {
        self.host = None;
        let jar = self
            .settings
            .host_jar
            .clone()
            .or_else(|| crate::core::settings::autodetect_host_jar().map(|p| p.to_string_lossy().to_string()));
        let Some(jar) = jar else {
            self.host_status = HostStatus::Failed(
                "未找到 spider-host-all.jar。请先运行 gradlew :spider-host:fatJar，或在设置页填写 jar 路径。".into(),
            );
            return;
        };
        self.host_status = HostStatus::Starting;
        let jvm_arg = format!("-Xmx{}m", self.settings.host_memory_mb);
        match SpiderHost::spawn(&self.settings.java_path, &jar, &[jvm_arg]) {
            Ok(h) => {
                self.host = Some(h);
                // hello 异步到达，由 check_host 提升为 Ready
                if self.settings.active_source().is_some() {
                    self.request_load_config();
                }
            }
            Err(e) => {
                self.host_status = HostStatus::Failed(format!("{e:#}"));
            }
        }
    }

    pub fn restart_host(&mut self) {
        if let Some(h) = &self.host {
            h.shutdown();
        }
        self.start_host();
    }

    fn check_host(&mut self) {
        let starting = matches!(self.host_status, HostStatus::Starting);
        if starting {
            if let Some(h) = &self.host {
                if let Some(info) = h.hello() {
                    self.host_status = HostStatus::Ready(info);
                }
            }
        }
        let dead = self.host.as_ref().map(|h| !h.is_alive()).unwrap_or(false);
        if dead && !matches!(self.host_status, HostStatus::Failed(_)) {
            self.host_status = HostStatus::Failed("宿主进程已退出".into());
            self.host = None;
        }
    }

    // ---------- 配置 ----------

    pub fn request_load_config(&mut self) {
        let Some(src) = self.settings.active_source().cloned() else {
            self.toast("请先添加配置源");
            return;
        };
        let Some(host) = self.host.clone() else {
            self.toast("宿主未就绪，无法加载配置");
            return;
        };
        let tx = self.event_tx.clone();
        self.home.reset();
        self.toast("正在加载配置…");
        std::thread::spawn(move || {
            let res: anyhow::Result<(RawApiConfig, String)> = (|| {
                let raw = crate::core::config::fetch_config(&src)?;
                let cfg_id = crate::core::config::md5_hex(&raw);
                let url = match crate::core::config::classify(&src) {
                    crate::core::config::SourceKind::Http(u) => u,
                    _ => String::new(),
                };
                let normalized = host.request(
                    "loadConfig",
                    json!({"data": raw, "url": url, "cfgId": cfg_id}),
                    Duration::from_secs(60),
                )?;
                let norm_str = match normalized {
                    Value::String(s) => s,
                    o => o.to_string(),
                };
                let cfg: RawApiConfig = serde_json::from_str(&norm_str)?;
                Ok((cfg, cfg_id))
            })();
            match res {
                Ok((cfg, id)) => {
                    let _ = tx.send(AppEvent::ConfigReady(cfg, id));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::ConfigError(format!("{e:#}")));
                }
            }
        });
    }

    // ---------- 首页 ----------

    pub fn select_site(&mut self, site: crate::core::models::Site) {
        self.home.reset();
        self.home.site_key = Some(site.key.clone());
        self.home.site_name = site.name.clone();
        self.home.loading = true;
        let Some(host) = self.host.clone() else {
            self.home.loading = false;
            self.toast("宿主未就绪");
            return;
        };
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            // proxyLocal 需要知道最近使用的 jar
            let _ = host.request("setRecent", json!({"siteKey": site.key}), Duration::from_secs(5));
            match host.request_spider_result("homeContent", json!({"siteKey": site.key, "filter": true}), Duration::from_secs(60)) {
                Ok(r) => {
                    let _ = tx.send(AppEvent::HomeTypes(r));
                    match host.request_spider_result("homeVideoContent", json!({"siteKey": site.key}), Duration::from_secs(60)) {
                        Ok(r2) => {
                            let _ = tx.send(AppEvent::HomeVideos(r2));
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Toast(format!("推荐内容加载失败: {e:#}")));
                            let _ = tx.send(AppEvent::HomeVideos(SpiderResult::default()));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Toast(format!("分类加载失败: {e:#}")));
                    let _ = tx.send(AppEvent::HomeTypes(SpiderResult::default()));
                    let _ = tx.send(AppEvent::HomeVideos(SpiderResult::default()));
                }
            }
        });
    }

    pub fn select_category(&mut self, idx: usize) {
        self.home.selected_type = idx;
        let Some(site_key) = self.home.site_key.clone() else { return };
        let Some(host) = self.host.clone() else { return };
        let tx = self.event_tx.clone();

        if idx == 0 {
            // 推荐
            self.home.loading = true;
            self.home.vods.clear();
            self.home.page = 1;
            self.home.pagecount = 1;
            std::thread::spawn(move || {
                match host.request_spider_result("homeVideoContent", json!({"siteKey": site_key}), Duration::from_secs(60)) {
                    Ok(r) => {
                        let _ = tx.send(AppEvent::HomeVideos(r));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Toast(format!("加载失败: {e:#}")));
                        let _ = tx.send(AppEvent::HomeVideos(SpiderResult::default()));
                    }
                }
            });
            return;
        }

        let Some(t) = self.home.types.get(idx) else { return };
        let tid = t.type_id.clone();
        let filters = self.home.filters.get(&tid).cloned().unwrap_or_default();
        let mut extend = serde_json::Map::new();
        for f in &filters {
            if let Some(key) = &f.key {
                if let Some(v) = self.home.choice.get(key) {
                    extend.insert(key.clone(), json!(v.clone()));
                }
            }
        }
        self.home.loading = true;
        self.home.vods.clear();
        self.home.page = 1;
        std::thread::spawn(move || {
            match host.request_spider_result(
                "categoryContent",
                json!({"siteKey": site_key, "tid": tid, "pg": "1", "filter": true, "extend": extend}),
                Duration::from_secs(60),
            ) {
                Ok(r) => {
                    let _ = tx.send(AppEvent::CategoryPage { page: 1, result: r });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Toast(format!("分类加载失败: {e:#}")));
                    let _ = tx.send(AppEvent::CategoryPage { page: 1, result: SpiderResult::default() });
                }
            }
        });
    }

    pub fn load_more_category(&mut self) {
        if self.home.selected_type == 0 {
            return; // 推荐流无分页
        }
        let Some(site_key) = self.home.site_key.clone() else { return };
        let Some(host) = self.host.clone() else { return };
        let Some(t) = self.home.types.get(self.home.selected_type) else { return };
        let tid = t.type_id.clone();
        let filters = self.home.filters.get(&tid).cloned().unwrap_or_default();
        let mut extend = serde_json::Map::new();
        for f in &filters {
            if let Some(key) = &f.key {
                if let Some(v) = self.home.choice.get(key) {
                    extend.insert(key.clone(), json!(v.clone()));
                }
            }
        }
        let next = self.home.page + 1;
        self.home.loading = true;
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            match host.request_spider_result(
                "categoryContent",
                json!({"siteKey": site_key, "tid": tid, "pg": next.to_string(), "filter": true, "extend": extend}),
                Duration::from_secs(60),
            ) {
                Ok(r) => {
                    let _ = tx.send(AppEvent::CategoryPage { page: next, result: r });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Toast(format!("加载失败: {e:#}")));
                    let _ = tx.send(AppEvent::CategoryPage { page: next, result: SpiderResult::default() });
                }
            }
        });
    }

    // ---------- 搜索 ----------

    pub fn do_search(&mut self) {
        let q = self.search.input.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.search.reset(q.clone());
        let sites: Vec<crate::core::models::Site> = self
            .config
            .as_ref()
            .map(|c| c.sites.iter().filter(|s| s.is_searchable()).cloned().collect())
            .unwrap_or_default();
        if sites.is_empty() {
            self.toast("当前配置没有可搜索的站点");
            return;
        }
        self.search.sites = sites;
        self.search_more();
    }

    pub fn search_more(&mut self) {
        let batch = self.settings.search_batch.max(1);
        let Some(host) = self.host.clone() else { return };
        let tx = self.event_tx.clone();
        let take: Vec<crate::core::models::Site> = self
            .search
            .sites
            .iter()
            .skip(self.search.next_batch)
            .take(batch)
            .cloned()
            .collect();
        if take.is_empty() {
            return;
        }
        self.search.next_batch += take.len();
        self.search.pending += take.len();
        let query = self.search.query.clone();
        for site in take {
            let host = host.clone();
            let tx = tx.clone();
            let q = query.clone();
            std::thread::spawn(move || {
                let res = host.request_spider_result(
                    "searchContent",
                    json!({"siteKey": site.key, "key": q, "quick": false, "pg": "1"}),
                    Duration::from_secs(90),
                );
                let ev = match res {
                    Ok(r) => AppEvent::SearchSiteResult {
                        site_key: site.key.clone(),
                        site_name: site.name.clone(),
                        vods: r.list,
                        error: None,
                    },
                    Err(e) => AppEvent::SearchSiteResult {
                        site_key: site.key.clone(),
                        site_name: site.name.clone(),
                        vods: vec![],
                        error: Some(format!("{e:#}")),
                    },
                };
                let _ = tx.send(ev);
            });
        }
    }

    // ---------- 详情 / 播放 ----------

    pub fn open_detail(&mut self, site_key: String, vod_id: String) {
        if self.screen != Screen::Detail {
            self.detail_back = self.screen;
        }
        self.screen = Screen::Detail;
        self.detail = DetailState {
            site_key: site_key.clone(),
            loading: true,
            ..Default::default()
        };
        let Some(host) = self.host.clone() else {
            self.detail.loading = false;
            self.detail.error = Some("宿主未运行".into());
            return;
        };
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            match host.request_spider_result(
                "detailContent",
                json!({"siteKey": site_key, "ids": [vod_id]}),
                Duration::from_secs(60),
            ) {
                Ok(r) => {
                    let _ = tx.send(AppEvent::DetailLoaded { result: r });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::DetailError(format!("{e:#}")));
                }
            }
        });
    }

    pub fn play_episode(&mut self, flag_name: &str, ep: crate::core::models::Episode) {
        if self.detail.playing {
            return;
        }
        self.detail.playing = true;
        let Some(host) = self.host.clone() else {
            self.detail.playing = false;
            self.toast("宿主未运行");
            return;
        };
        let site_key = self.detail.site_key.clone();
        let vod = self.detail.vod.clone().unwrap_or_default();
        let flag = flag_name.to_string();
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let res = host.request_spider_result(
                "playerContent",
                json!({"siteKey": site_key, "flag": flag, "id": ep.url, "vipFlags": []}),
                Duration::from_secs(45),
            );
            match res {
                Ok(r) => {
                    let url = r.url.v();
                    if url.is_empty() {
                        let _ = tx.send(AppEvent::PlayError("播放地址为空（线路可能已失效）".into()));
                    } else {
                        let _ = tx.send(AppEvent::PlayReady {
                            url,
                            headers: r.header.clone().unwrap_or_default(),
                            site_key: site_key.clone(),
                            vod_id: vod.vod_id.clone(),
                            vod_name: vod.vod_name.clone(),
                            vod_pic: vod.vod_pic.clone(),
                            flag,
                            ep_url: ep.url.clone(),
                            ep_name: ep.name.clone(),
                        });
                    }
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::PlayError(format!("{e:#}")));
                }
            }
        });
    }

    // ---------- 设置 ----------

    pub fn apply_settings_edits(&mut self) {
        self.settings.player_path = self.settings_page.player_path_edit.trim().to_string();
        self.settings.java_path = {
            let j = self.settings_page.java_path_edit.trim().to_string();
            if j.is_empty() { "java".into() } else { j }
        };
        let jar = self.settings_page.host_jar_edit.trim().to_string();
        self.settings.host_jar = if jar.is_empty() { None } else { Some(jar) };
        self.settings.save();
        self.settings_page.dirty = false;
        self.toast("设置已保存");
    }

    // ---------- 通用 ----------

    pub fn toast<S: Into<String>>(&mut self, msg: S) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn process_events(&mut self) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                AppEvent::ConfigReady(cfg, id) => {
                    let n = cfg.sites.len();
                    self.config = Some(cfg);
                    self.cfg_id = id;
                    self.toast(format!("配置加载完成，{} 个站点", n));
                    self.home.reset();
                    if let Some(first) = self.config.as_ref().and_then(|c| c.sites.first().cloned()) {
                        self.select_site(first);
                    }
                }
                AppEvent::ConfigError(e) => {
                    self.toast(format!("配置加载失败: {}", e));
                }
                AppEvent::HomeTypes(r) => {
                    let mut types = vec![Type { type_id: "home".into(), type_name: "推荐".into() }];
                    types.extend(r.types.clone());
                    self.home.types = types;
                    self.home.filters = r.filters.clone();
                    self.home.selected_type = 0;
                }
                AppEvent::HomeVideos(r) => {
                    self.home.vods = r.list.clone();
                    self.home.page = 1;
                    self.home.pagecount = r.pagecount.unwrap_or(1).max(1);
                    self.home.loading = false;
                }
                AppEvent::CategoryPage { page, result } => {
                    if page <= 1 {
                        self.home.vods = result.list.clone();
                    } else {
                        self.home.vods.extend(result.list.clone());
                    }
                    self.home.page = page.max(1);
                    self.home.pagecount = result.pagecount.unwrap_or(page).max(self.home.page);
                    self.home.loading = false;
                }
                AppEvent::SearchSiteResult { site_key, site_name, vods, error } => {
                    self.search.pending = self.search.pending.saturating_sub(1);
                    if let Some(e) = error {
                        self.search.error_count += 1;
                        self.toast(format!("{}: {}", site_name, e));
                    }
                    for v in vods {
                        self.search.results.push(SearchHit {
                            site_key: site_key.clone(),
                            site_name: site_name.clone(),
                            vod: v,
                        });
                    }
                }
                AppEvent::DetailLoaded { result } => {
                    self.detail.loading = false;
                    match result.list.first() {
                        Some(vod) => {
                            let flags = crate::core::models::build_flags(&vod.vod_play_from, &vod.vod_play_url);
                            if flags.is_empty() {
                                self.detail.error = Some("无可用播放线路".into());
                            } else {
                                self.detail.flags = flags;
                                self.detail.flag_idx = 0;
                                self.detail.ep_page = 0;
                                self.detail.vod = Some(vod.clone());
                            }
                        }
                        None => {
                            self.detail.error = Some("详情数据为空".into());
                        }
                    }
                }
                AppEvent::DetailError(e) => {
                    self.detail.loading = false;
                    self.detail.error = Some(e);
                }
                AppEvent::PlayReady { url, headers, site_key, vod_id, vod_name, vod_pic, flag, ep_url, ep_name } => {
                    self.detail.playing = false;
                    let player_path = {
                        let configured = self.settings.player_path.trim().to_string();
                        if configured.is_empty() {
                            crate::core::settings::detect_players().first().map(|d| d.path.clone()).unwrap_or_default()
                        } else {
                            configured
                        }
                    };
                    let title = if ep_name.is_empty() { vod_name.clone() } else { format!("{} - {}", vod_name, ep_name) };
                    match crate::core::player::launch(&player_path, &url, &title, &headers) {
                        Ok(()) => {
                            self.history_store.upsert(HistoryEntry {
                                key: format!("{}@@@{}@@@{}", site_key, vod_id, self.cfg_id),
                                site_key,
                                vod_id,
                                cfg_id: self.cfg_id.clone(),
                                vod_name,
                                vod_pic,
                                vod_flag: flag,
                                ep_name: ep_name.clone(),
                                episode_url: ep_url,
                                create_time: now_millis(),
                            });
                            self.toast(format!("已启动外部播放器: {}", ep_name));
                        }
                        Err(e) => {
                            self.toast(format!("{e:#}"));
                        }
                    }
                }
                AppEvent::PlayError(e) => {
                    self.detail.playing = false;
                    self.toast(e);
                }
                AppEvent::Toast(msg) => {
                    self.toast(msg);
                }
            }
        }
    }

    fn draw_top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("main_tabs").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("TV Rust").color(theme::ACCENT).size(18.0).strong());
                ui.separator();
                let tabs = [
                    (Screen::Home, "首页"),
                    (Screen::Search, "搜索"),
                    (Screen::History, "历史"),
                    (Screen::Settings, "设置"),
                ];
                for (s, label) in tabs {
                    let selected = self.screen == s;
                    if ui.selectable_label(selected, egui::RichText::new(label).color(if selected { theme::ACCENT } else { theme::TEXT })).clicked() {
                        self.screen = s;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(egui::RichText::new("↻ 重新加载配置").size(12.0).color(theme::ACCENT)).clicked() {
                        self.request_load_config();
                    }
                    match &self.host_status {
                        HostStatus::Starting => ui.label(egui::RichText::new("● 宿主启动中").color(theme::TEXT_DIM).size(12.0)),
                        HostStatus::Ready(info) => ui.label(egui::RichText::new(format!("● {}", info)).color(theme::ACCENT).size(12.0)),
                        HostStatus::Failed(e) => ui.label(egui::RichText::new(format!("● {}", e)).color(theme::DANGER).size(12.0)),
                    };
                    let site = self.home.site_name.clone();
                    if !site.is_empty() && matches!(self.screen, Screen::Home | Screen::Detail) {
                        ui.label(egui::RichText::new(format!("当前站源: {}", site)).color(theme::TEXT_DIM).size(12.0));
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let expired = self
            .toast
            .as_ref()
            .map(|(_, t)| t.elapsed() > Duration::from_secs(3))
            .unwrap_or(true);
        if expired {
            self.toast = None;
            return;
        }
        if let Some((msg, _)) = &self.toast {
            egui::Area::new(egui::Id::new("toast_area"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -28.0])
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(theme::BG_PANEL)
                        .stroke(egui::Stroke::new(1.0, theme::ACCENT))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(msg).color(theme::TEXT));
                        });
                });
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl eframe::App for TVApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.textures.poll(ctx);
        self.check_host();
        self.process_events();

        self.draw_top_panel(ctx);
        match self.screen {
            Screen::Home => self.ui_home(ctx),
            Screen::Search => self.ui_search(ctx),
            Screen::Detail => self.ui_detail(ctx),
            Screen::History => self.ui_history(ctx),
            Screen::Settings => self.ui_settings(ctx),
        }
        self.draw_toast(ctx);

        // 有异步事件/纹理下载时保持刷新
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

impl Drop for TVApp {
    fn drop(&mut self) {
        if let Some(h) = &self.host {
            h.shutdown();
        }
    }
}
