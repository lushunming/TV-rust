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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
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
    /// 热搜榜（首页顶栏搜索框轮播提示的数据源）
    HotList(Vec<crate::core::hot::HotData>),
    /// 搜索联想词（query 用于丢弃过期响应）
    Suggest { query: String, items: Vec<String> },
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

/// 搜索框轮播提示 + 热搜数据（对应原 Kotlin `VideoScreenState.prompt` / `isRunning`）
pub struct HotState {
    pub items: Vec<crate::core::hot::HotData>,
    /// 当前轮播到第几条
    pub idx: usize,
    /// 当前显示的提示文案
    pub prompt: String,
    /// 下一次切换时刻；None 表示尚未开始轮播
    pub next_at: Option<Instant>,
}

impl Default for HotState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            idx: 0,
            prompt: String::new(),
            next_at: None,
        }
    }
}

pub struct TVApp {
    pub settings: Settings,
    pub host: Option<Arc<SpiderHost>>,
    pub host_status: HostStatus,
    /// 宿主内置代理服务根地址（hello 下发），用于重写 127.0.0.1:9978 的本地 URL
    pub proxy_base: Option<String>,
    pub config: Option<RawApiConfig>,
    pub cfg_id: String,

    pub event_tx: Sender<AppEvent>,
    pub event_rx: Receiver<AppEvent>,

    pub screen: Screen,
    pub detail_back: Screen,

    pub home: HomeState,
    pub hot: HotState,
    pub search: SearchState,
    pub detail: DetailState,
    pub history_store: HistoryStore,
    pub settings_page: SettingsPageState,

    pub textures: TextureCache,
    pub toast: Option<(String, Instant)>,
    /// 已提示过 init 告警的站点，避免每次刷新都弹同一条
    pub warned_sites: std::collections::HashSet<String>,

    /// 内置（内嵌）mpv 播放器实例；None 表示未在窗口内播放
    pub internal: Option<crate::core::internal_player::InternalPlayer>,
    /// 当前播放地址对应的 HTTP 头（重试/重播时复用）
    pub internal_headers: Vec<(String, String)>,
    /// 内置播放层 UI 状态
    pub player_overlay: crate::screens::player::PlayerOverlay,
    /// 上次 ESC 键是否处于按下状态（用于 Win32 GetAsyncKeyState 防抖）
    pub esc_was_down: bool,
    /// 专用 ESC 检测线程的原子标志（15ms 轮询，不会漏掉快速按键）
    pub esc_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// egui 上下文（用于在非 update 回调中发 ViewportCommand，例如退出全屏）
    pub egui_ctx: Option<egui::Context>,
    /// 上次处理 ESC 的时刻：两次 ESC 之间需间隔，避免同一次按键被
    /// "全局轮询线程 + egui 输入"同时检出而连续触发两次（先退全屏、再关播放）
    pub last_esc: Option<std::time::Instant>,
    /// 内置播放器组件（libmpv）的按需下载安装器
    pub libmpv_installer: crate::core::libmpv_install::LibMpvInstaller,
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
            libmpv_installer: crate::core::libmpv_install::LibMpvInstaller::new(),
            host: None,
            host_status: HostStatus::Starting,
            proxy_base: None,
            config: None,
            cfg_id: String::new(),
            event_tx: tx,
            event_rx: rx,
            screen: Screen::Home,
            detail_back: Screen::Home,
            home: HomeState::default(),
            hot: HotState::default(),
            search: SearchState::default(),
            detail: DetailState::default(),
            history_store: HistoryStore::load(),
            textures: TextureCache::new(),
            toast: None,
            warned_sites: std::collections::HashSet::new(),
            internal: None,
            internal_headers: Vec::new(),
            player_overlay: crate::screens::player::PlayerOverlay::default(),
            esc_was_down: false,
            esc_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            egui_ctx: Some(cc.egui_ctx.clone()),
            last_esc: None,
        };
        // settings_page 已在上面初始化，重新同步编辑框
        app.settings_page.player_path_edit = app.settings.player_path.clone();
        app.settings_page.java_path_edit = app.settings.java_path.clone();
        app.settings_page.host_jar_edit = app.settings.host_jar.clone().unwrap_or_default();
        app.start_host();
        app.load_hot();
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
                "未找到 spider-host-all.jar。打包版请确认 jar 与 exe 同目录；开发版请运行 gradlew :spider-host:fatJar，或在设置页填写 jar 路径。".into(),
            );
            return;
        };
        self.host_status = HostStatus::Starting;
        // 宿主跑在 JDK 25（LTS）上：G1 是服务端类机器的默认收集器，
        // 这里再补三件事——小初始堆、空闲时把堆还给系统、字符串去重，
        // 让长时间挂着看剧时宿主的内存曲线往下走而不是只涨不降。
        let heap = self.settings.host_memory_mb.max(128);
        let initial = 16.min(heap);
        let jvm_args = vec![
            format!("-Xms{initial}m"),
            format!("-Xmx{heap}m"),
            "-XX:+UseG1GC".to_string(),
            "-XX:G1PeriodicGCInterval=300000".to_string(),
            "-XX:+UseStringDeduplication".to_string(),
            "-XX:+ExitOnOutOfMemoryError".to_string(),
        ];
        let java = crate::core::settings::resolve_java(&self.settings.java_path);
        crate::core::log::info(format!(
            "starting host: java={} jar={} heap={}m args={:?}",
            java, jar, heap, jvm_args
        ));
        match SpiderHost::spawn(&java, &jar, &jvm_args) {
            Ok(h) => {
                self.host = Some(h);
                crate::core::log::info("spider-host 进程已创建，等待 hello");
                // hello 异步到达，由 check_host 提升为 Ready
                if self.settings.active_source().is_some() {
                    self.request_load_config();
                }
            }
            Err(e) => {
                crate::core::log::error(format!("启动宿主失败: {e:#}"));
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
                    crate::core::log::info(format!("host ready: {}", info.trim()));
                    self.host_status = HostStatus::Ready(info);
                    self.proxy_base = h.proxy_base();
                }
            }
        }
        let dead = self.host.as_ref().map(|h| !h.is_alive()).unwrap_or(false);
        if dead && !matches!(self.host_status, HostStatus::Failed(_)) {
            crate::core::log::error("宿主进程已退出（详见 spider-host.log）");
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
        crate::core::log::info(format!("loading config from {}", src));
        self.home.reset();
        self.toast("正在加载配置…");
        std::thread::spawn(move || {
            let res: anyhow::Result<(RawApiConfig, String, crate::core::config::JarPrefetch)> = (|| {
                let raw = crate::core::config::fetch_config(&src)?;
                let cfg_id = crate::core::config::md5_hex(&raw);
                let url = match crate::core::config::classify(&src) {
                    crate::core::config::SourceKind::Http(u) => u,
                    _ => String::new(),
                };
                let normalized = host.request(
                    "loadConfig",
                    json!({"data": raw, "url": url.clone(), "cfgId": cfg_id}),
                    Duration::from_secs(60),
                )?;
                let norm_str = match normalized {
                    Value::String(s) => s,
                    o => o.to_string(),
                };
                let cfg: RawApiConfig = serde_json::from_str(&norm_str)?;
                // 宿主 JVM 侧下载 jar 可能被服务端 TLS 指纹拒绝（handshake_failure），
                // 先用 Rust 的 TLS 栈把 jar 写入宿主缓存目录，保证 csp_ 站点有数据。
                let report = crate::core::config::prefetch_spider_jars(&url, &cfg);
                Ok((cfg, cfg_id, report))
            })();
            match res {
                Ok((cfg, id, report)) => {
                    let _ = tx.send(AppEvent::ConfigReady(cfg, id));
                    if !report.failed.is_empty() {
                        let _ = tx.send(AppEvent::Toast(format!(
                            "爬虫 jar 预取失败 {} 个，相关站点可能无数据：{}",
                            report.failed.len(),
                            report.failed[0]
                        )));
                    }
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::ConfigError(format!("{e:#}")));
                }
            }
        });
    }

    // ---------- 热搜（顶栏搜索框提示） ----------

    /// 先用磁盘缓存立即可用，再后台刷新（与 Kotlin `Hot.getHotList()` 一致）
    pub fn load_hot(&mut self) {
        let cached = crate::core::hot::load_cache();
        if !cached.is_empty() {
            crate::core::log::info(format!("热搜缓存 {} 条", cached.len()));
            self.hot.items = cached;
        }
        let tx = self.event_tx.clone();
        std::thread::spawn(move || match crate::core::hot::fetch() {
            Ok(list) if !list.is_empty() => {
                let _ = tx.send(AppEvent::HotList(list));
            }
            Ok(_) => {}
            Err(e) => crate::core::log::warn(format!("热搜拉取失败: {e:#}")),
        });
    }

    // ---------- 首页 ----------

    pub fn select_site(&mut self, site: crate::core::models::Site) {
        crate::core::log::info(format!("select site: {} ({})", site.name, site.key));
        self.home.reset();
        self.home.site_key = Some(site.key.clone());
        self.home.site_name = site.name.clone();
        // 记住上次使用的站源：重启后直接恢复，而不是回到配置里第一个（往往是网盘/索引站）
        if self.settings.last_site_key.as_deref() != Some(site.key.as_str()) {
            self.settings.last_site_key = Some(site.key.clone());
            self.settings.save();
        }
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
            self.home.loading_more = false;
            self.home.no_more = false;
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
        self.home.loading_more = false;
        self.home.no_more = false;
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
        if self.home.loading || self.home.loading_more || self.home.no_more {
            return; // 已在加载或已到底，避免滚动时重复触发
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
        self.home.loading_more = true;
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

    /// 把爬虫硬编码的本地代理地址（127.0.0.1:9978）重写为宿主实际监听地址。
    /// 端口无冲突时不发生任何改动。
    pub fn localize_url(&self, url: &str) -> String {
        let Some(base) = &self.proxy_base else {
            return url.to_string();
        };
        let mut out = url.to_string();
        for host in ["127.0.0.1:9978", "localhost:9978"] {
            if out.contains(host) {
                out = out
                    .replace(&format!("http://{host}"), base)
                    .replace(&format!("https://{host}"), base);
            }
        }
        out
    }

    /// 站点 header（图片、播放地址复用），来自配置里的 site.header
    pub fn site_headers(&self, site_key: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(site) = self.config.as_ref().and_then(|c| c.sites.iter().find(|s| s.key == site_key)) {
            if let Some(h) = &site.header {
                out.extend(h.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
        out
    }

    /// 海报纹理：自动带上站点 header，并重写本地代理 URL
    pub fn poster_texture(&mut self, pic: &str, site_key: &str) -> Option<egui::TextureHandle> {
        if pic.is_empty() {
            return None;
        }
        let url = self.localize_url(pic);
        let headers = self.site_headers(site_key);
        self.textures.get_with_headers(&url, &headers)
    }

    /// 站点 init 告警：每个站点只提示一次（仍会完整写入日志）
    fn note_site_warning(&mut self, warning: Option<String>) {
        let Some(w) = warning else { return };
        let key = self.home.site_key.clone().unwrap_or_default();
        let name = self.home.site_name.clone();
        crate::core::log::warn(format!("site warning {}: {}", key, w));
        if !key.is_empty() && !self.warned_sites.insert(key) {
            return;
        }
        self.toast(format!("{} 部分功能不可用：{}", name, w));
    }

    pub fn toast<S: Into<String>>(&mut self, msg: S) {
        let text = msg.into();
        // 所有用户可见提示都落盘：抓不到问题的时候可以直接 grep 日志
        if text.contains("失败") || text.contains("无法") || text.contains("异常") || text.contains("错误") {
            crate::core::log::warn(format!("toast: {text}"));
        } else {
            crate::core::log::info(format!("toast: {text}"));
        }
        self.toast = Some((text, Instant::now()));
    }

    fn process_events(&mut self) {
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                AppEvent::ConfigReady(cfg, id) => {
                    let n = cfg.sites.len();
                    crate::core::log::info(format!("config ready: cfgId={} sites={}", id, n));
                    self.config = Some(cfg);
                    self.cfg_id = id;
                    self.toast(format!("配置加载完成，{} 个站点", n));
                    self.home.reset();
                    let preferred = self.settings.last_site_key.clone().and_then(|k| {
                        self.config.as_ref().and_then(|c| c.sites.iter().find(|s| s.key == k).cloned())
                    });
                    let site = preferred.or_else(|| self.config.as_ref().and_then(|c| c.sites.first().cloned()));
                    if let Some(site) = site {
                        self.select_site(site);
                    }
                }
                AppEvent::ConfigError(e) => {
                    crate::core::log::error(format!("config error: {e}"));
                    self.toast(format!("配置加载失败: {}", e));
                }
                AppEvent::HomeTypes(r) => {
                    let warn = r.warning.clone();
                    self.note_site_warning(warn);
                    // homeContent 不仅给分类，通常还带着一批推荐内容（Wogg 255 条、SeedHub 20 条）；
                    // 以前只取 class 把 list 丢掉，首页只能退回 homeVideoContent，而很多爬虫该接口返回空 → 看起来“没视频”
                    let recommended = r.list.clone();
                    crate::core::log::info(format!(
                        "home recommended: {} items, {} types, site={}",
                        recommended.len(),
                        r.types.len(),
                        self.home.site_name
                    ));
                    if !recommended.is_empty() {
                        self.home.vods = recommended;
                        self.home.page = 1;
                        self.home.pagecount = r.pagecount.unwrap_or(1).max(1);
                        self.home.loading = false;
                    }
                    let mut types = vec![Type { type_id: "home".into(), type_name: "推荐".into() }];
                    types.extend(r.types.clone());
                    self.home.types = types;
                    self.home.filters = r.filters.clone();
                    self.home.selected_type = 0;
                    // 应用筛选默认值（Filter.init）：首屏展示的选中状态必须与后端实际使用的参数一致
                    self.home.choice.clear();
                    for group in self.home.filters.values() {
                        for f in group {
                            if let Some(key) = &f.key {
                                if !f.init.trim().is_empty() {
                                    self.home.choice.insert(key.clone(), f.init.clone());
                                }
                            }
                        }
                    }
                }
                AppEvent::HomeVideos(r) => {
                    let warn = r.warning.clone();
                    self.note_site_warning(warn);
                    crate::core::log::info(format!(
                        "home videos: {} site={} pagecount={:?}",
                        r.list.len(),
                        self.home.site_name,
                        r.pagecount
                    ));
                    // homeVideoContent 为空时不要清掉 homeContent 带来的推荐列表
                    if !r.list.is_empty() || self.home.vods.is_empty() {
                        self.home.vods = r.list.clone();
                        self.home.page = 1;
                        self.home.pagecount = r.pagecount.unwrap_or(1).max(1);
                    }
                    crate::core::log::info(format!("home grid after homeVideoContent: {} items", self.home.vods.len()));
                    self.home.loading = false;
                }
                AppEvent::CategoryPage { page, result } => {
                    crate::core::log::info(format!(
                        "category page: p{} items={} site={}",
                        page,
                        result.list.len(),
                        self.home.site_name
                    ));
                    if page <= 1 {
                        self.home.vods = result.list.clone();
                        self.home.no_more = false;
                    } else if result.list.is_empty() {
                        // 下一页为空：视为已到末尾，避免滚动加载反复请求同一页
                        self.home.no_more = true;
                    } else {
                        self.home.vods.extend(result.list.clone());
                    }
                    self.home.page = page.max(1);
                    // 站点未上报 pagecount 时不要直接当作到底：
                    // 有内容就假定还有下一页，靠"下一页为空"来结束（否则永远无法滚动加载）
                    let reported = result.pagecount.unwrap_or(0);
                    self.home.pagecount = if reported > 0 {
                        reported.max(self.home.page)
                    } else if result.list.is_empty() {
                        self.home.page
                    } else {
                        self.home.page + 1
                    };
                    self.home.loading = false;
                    self.home.loading_more = false;
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
                    crate::core::log::warn(format!("detail error: {e}"));
                    self.detail.loading = false;
                    self.detail.error = Some(e);
                }
                AppEvent::PlayReady { url, headers, site_key, vod_id, vod_name, vod_pic, flag, ep_url, ep_name } => {
                    self.detail.playing = false;
                    let url = self.localize_url(&url);
                    let title = if ep_name.is_empty() { vod_name.clone() } else { format!("{} - {}", vod_name, ep_name) };

                    // 无论内置还是外部播放，都先记历史：失败时用户也能在历史里重新找到这一集
                    self.history_store.upsert(HistoryEntry {
                        key: format!("{}@@@{}@@@{}@{}", site_key, vod_id, self.cfg_id, ep_url),
                        site_key: site_key.clone(),
                        vod_id: vod_id.clone(),
                        cfg_id: self.cfg_id.clone(),
                        vod_name: vod_name.clone(),
                        vod_pic: vod_pic.clone(),
                        vod_flag: flag.clone(),
                        ep_name: ep_name.clone(),
                        episode_url: ep_url.clone(),
                        create_time: now_millis(),
                    });

                    let configured = self.settings.player_path.trim();
                    if !configured.is_empty() {
                        // 用户配置了外部播放器路径 → 优先使用外部
                        match crate::core::player::launch(configured, &url, &title, &headers) {
                            Ok(()) => {
                                self.toast(format!("已启动外部播放器: {}", ep_name));
                            }
                            Err(e) => {
                                self.toast(format!("{e:#}"));
                            }
                        }
                    } else {
                        // 未配置外部播放器 → 使用内置 libmpv
                        let overlay = crate::screens::player::PlayerOverlay {
                            visible: true,
                            title: title.clone(),
                            site_key: site_key.clone(),
                            vod_id: vod_id.clone(),
                            vod_name: vod_name.clone(),
                            vod_pic: vod_pic.clone(),
                            flag: flag.clone(),
                            ep_url: ep_url.clone(),
                            ep_name: ep_name.clone(),
                            url: url.clone(),
                            back: self.screen,
                            volume: self.settings.internal_volume,
                            ..Default::default()
                        };
                        let header_pairs: Vec<(String, String)> = headers
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        self.internal_headers = header_pairs.clone();
                        self.start_internal_player(url, header_pairs, overlay);
                    }
                }
                AppEvent::PlayError(e) => {
                    crate::core::log::warn(format!("play error: {e}"));
                    self.detail.playing = false;
                    self.toast(e);
                }
                AppEvent::HotList(list) => {
                    crate::core::log::info(format!("热搜 {} 条", list.len()));
                    self.hot.items = list;
                    self.hot.idx = 0;
                    self.hot.prompt.clear();
                    self.hot.next_at = None;
                }
                AppEvent::Suggest { query, items } => {
                    self.search.suggest.absorb(query, items);
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
        self.check_host();
        self.process_events();
        // 抽千 libmpv 下载器回传的状态（进度/完成/失败）
        self.libmpv_installer.poll();

        // 内置播放层：接管整个窗口，避免爬虫/详情页在播放时还在后台刷新
        if self.player_overlay.visible {
            self.sync_internal_player();

            // ESC 返回：全屏时先退出全屏，否则关闭播放层
            // 使用专用线程的原子标志（15ms 轮询，即使窗口失焦也能检测到）
            //
            // 注意：同一帧里可能 egui 输入和轮询线程都报出这次 ESC，
            // 如果不去重，就会先退全屏、下一帧再当成"新的一次 ESC"关掉播放。
            // 所以用一个 700ms 的冷却窗口，同一次按键只算一次。
            {
                let esc_flag = self.esc_flag.load(std::sync::atomic::Ordering::SeqCst);
                let egui_esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                if esc_flag || egui_esc {
                    // 无论是否因冷却被忽略，都要清掉标志，避免残留到下一帧
                    self.esc_flag.store(false, std::sync::atomic::Ordering::SeqCst);

                    let now = std::time::Instant::now();
                    let too_soon = self
                        .last_esc
                        .map(|t| now.duration_since(t) < Duration::from_millis(700))
                        .unwrap_or(false);
                    if !too_soon {
                        self.last_esc = Some(now);
                        if self.player_overlay.fullscreen {
                            // 全屏时 ESC 只退全屏，绝不关播放
                            self.toggle_fullscreen(ctx);
                        } else {
                            self.stop_internal_player();
                        }
                    }
                }
            }
            // 空格暂停/恢复
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                self.toggle_internal_pause();
            }
            // 左右方向键 ±10s
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                self.seek_internal_relative(-10.0);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                self.seek_internal_relative(10.0);
            }
            // F11 全屏切换
            if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
                self.toggle_fullscreen(ctx);
            }

            self.ui_player_overlay(ctx);
            self.draw_toast(ctx);
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }

        self.textures.poll(ctx);
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
        if let Some(mut p) = self.internal.take() {
            p.stop();
        }
        if let Some(h) = &self.host {
            h.shutdown();
        }
    }
}
