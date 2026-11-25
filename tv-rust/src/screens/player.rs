//! 内置 mpv 播放层：占满窗口的视频区 + 底部控制条。
//!
//! 视频画面本身由 mpv 子进程渲染到一个 Win32 子窗口里（见 `core::internal_player`），
//! 因此这里只负责：
//!   1. 算好视频区矩形（含等比letterbox），把 mpv 子窗口摆到那里；
//!   2. 用 egui 画视频区以外的部分（控制条、加载/错误提示）。
//!
//! 关键点：mpv 子窗口是原生窗口，永远盖在 egui 绘制的像素之上，
//! 所以控制条必须放在视频区**之外**（底部独立一条），否则会被画面挡住。

use crate::app::TVApp;
use crate::theme::{ACCENT, BG, BG_PANEL, CARD, DANGER, TEXT, TEXT_DIM};
use egui::{Color32, RichText};

/// 播放层状态
#[derive(Default)]
pub struct PlayerOverlay {
    /// 是否处于播放层（true 时接管整个窗口绘制）
    pub visible: bool,
    /// 当前播放的标题（影片名 - 集名）
    pub title: String,
    /// 站点 / 线路 / 集名，用于返回详情页与记录
    pub site_key: String,
    pub vod_id: String,
    pub vod_name: String,
    pub vod_pic: String,
    pub flag: String,
    pub ep_url: String,
    pub ep_name: String,
    /// 原始（未重写）播放地址，重播时复用
    pub url: String,
    /// 加载中的提示文案；空表示已进入播放
    pub status: String,
    /// 播放失败信息
    pub error: Option<String>,
    /// 暂停状态（与 mpv 同步）
    pub paused: bool,
    /// 音量 0-100
    pub volume: u8,
    /// 当前播放位置 / 总时长（秒，0 表示未知）
    pub position: f64,
    pub duration: f64,
    /// 视频画面宽高比（w/h），None 表示未知（按铺满处理）
    pub aspect: Option<f32>,
    /// 上一次摆放下发的矩形（物理像素），用于跳过无变化的 SetWindowPos
    pub last_rect: Option<(i32, i32, i32, i32)>,
    /// 上次向 mpv 同步进度/比例的时刻（避免每帧都发 IPC）
    pub last_poll: Option<std::time::Instant>,
    /// 进入播放层之前的界面，返回时恢复
    pub back: crate::app::Screen,
    /// 是否全屏
    pub fullscreen: bool,
    /// 全屏前的窗口位置和尺寸（Win32 RECT）
    pub saved_window_rect: Option<(i32, i32, i32, i32)>,
    /// 缓冲进度 0.0-1.0
    pub cache_progress: f64,
}

impl TVApp {
    /// 内置播放器是否正在运行
    pub fn internal_playing(&self) -> bool {
        self.internal.is_some()
    }

    /// 启动内置播放器
    pub fn start_internal_player(
        &mut self,
        url: String,
        headers: Vec<(String, String)>,
        overlay: PlayerOverlay,
    ) {
        self.stop_internal_player();

        // 主窗口句柄：eframe 建窗后即可通过标题找到
        let parent = crate::core::internal_player::host_hwnd()
            .or_else(crate::core::internal_player::find_main_window)
            .unwrap_or(0);
        if parent == 0 {
            self.toast("找不到应用窗口句柄，无法内嵌播放；可改用外部播放器");
            self.player_overlay = PlayerOverlay::default();
            return;
        }
        crate::core::internal_player::set_host_hwnd(parent);

        // 自动查找打包的 libmpv DLL（exe 同级目录 / mpv/ 子目录）
        let libmpv_path = crate::core::settings::detect_libmpv()
            .unwrap_or_else(|| {
                // 打包形态：与 exe 放在同一目录
                if let Ok(exe) = std::env::current_exe() {
                    if let Some(dir) = exe.parent() {
                        for name in &["mpv-2.dll", "libmpv-2.dll", "mpv.dll"] {
                            let p = dir.join(name);
                            if p.is_file() {
                                return p.to_string_lossy().into_owned();
                            }
                        }
                    }
                }
                String::new()
            });
        if libmpv_path.is_empty() {
            #[cfg(windows)]
            let hint = "未找到 mpv-2.dll：请到「设置 → 播放器」点“下载内置播放器组件”（约 33 MB）";
            #[cfg(not(windows))]
            let hint = "未找到 libmpv.so.2：请安装 libmpv（如 sudo apt install libmpv2）";
            self.toast(hint);
            self.player_overlay = PlayerOverlay::default();
            return;
        }

        crate::core::log::info(format!(
            "内置播放器启动 url={} libmpv={} headers={}",
            url,
            libmpv_path,
            headers.len()
        ));

        let mut ov = overlay;
        ov.volume = self.settings.internal_volume.clamp(0, 100);
        ov.visible = true;
        ov.status = "正在缓冲…".into();

        match crate::core::internal_player::InternalPlayer::new(&libmpv_path, &url, &headers, parent) {
            Ok(p) => {
                p.set_volume(ov.volume);
                self.internal = Some(p);
                self.player_overlay = ov;

                // 启动 ESC 检测线程（15ms 轮询，即使 mpv 子窗口抢焦点也能检测到）
                #[cfg(windows)]
                {
                    use std::sync::atomic::Ordering;
                    use crate::core::internal_player::win;

                    // 整个进程只开一个轮询线程，避免反复播放时线程泄漏
                    static ESC_THREAD: std::sync::Once = std::sync::Once::new();
                    let flag = self.esc_flag.clone();
                    flag.store(false, Ordering::SeqCst);
                    ESC_THREAD.call_once(move || {
                        std::thread::Builder::new()
                            .name("esc-detect".into())
                            .spawn(move || {
                                let mut was_down = false;
                                loop {
                                    // 只在应用处于前台时才响应：否则在别的软件里按 ESC 也会误触发
                                    let focused = match crate::core::internal_player::host_hwnd() {
                                        Some(h) => unsafe {
                                            win::GetForegroundWindow() as isize == h
                                        },
                                        None => true,
                                    };
                                    let down = focused && win::is_key_pressed(win::VK_ESCAPE);
                                    // 只在“按下的那一瞬间”置位，长按不会反复触发
                                    if down && !was_down {
                                        flag.store(true, Ordering::SeqCst);
                                    }
                                    was_down = down;
                                    std::thread::sleep(std::time::Duration::from_millis(15));
                                }
                            })
                            .ok();
                    });
                }
            }
            Err(e) => {
                crate::core::log::error(format!("内置播放器启动失败: {e:#}"));
                ov.error = Some(format!("{e:#}"));
                ov.status.clear();
                self.player_overlay = ov;
            }
        }
    }

    /// 重播当前地址
    pub fn restart_internal_player(&mut self) {
        if self.player_overlay.url.is_empty() {
            return;
        }
        let mut ov = std::mem::take(&mut self.player_overlay);
        ov.error = None;
        ov.paused = false;
        ov.position = 0.0;
        ov.duration = 0.0;
        ov.aspect = None;
        ov.last_rect = None;
        let url = ov.url.clone();
        let headers = self.internal_headers.clone();
        self.start_internal_player(url, headers, ov);
    }

    /// 停止内置播放器并关闭播放层
    pub fn stop_internal_player(&mut self) {
        let was_fullscreen = self.player_overlay.fullscreen;
        if let Some(mut p) = self.internal.take() {
            crate::core::log::info("内置播放器停止");
            p.stop();
        }
        if self.player_overlay.visible {
            let back = self.player_overlay.back;
            self.player_overlay = PlayerOverlay::default();
            // 全屏状态下关闭播放层：必须把窗口恢复成普通窗口，否则会卡在全屏
            if was_fullscreen {
                if let Some(ctx) = &self.egui_ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    ctx.request_repaint();
                }
            }
            // 返回详情页（若有详情数据）
            if self.detail.vod.is_some() {
                self.screen = back;
            }
        }
    }

    /// 暂停/恢复
    pub fn toggle_internal_pause(&mut self) {
        if let Some(p) = self.internal.as_mut() {
            p.toggle_pause();
            self.player_overlay.paused = p.paused.load(std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// 相对跳转
    pub fn seek_internal_relative(&mut self, delta: f64) {
        if let Some(p) = self.internal.as_ref() {
            p.seek_relative(delta);
        }
    }

    /// 同步内置播放器状态（每帧调用）
    pub fn sync_internal_player(&mut self) {
        // 主窗口句柄补登记（首帧时窗口还不一定可见）
        if crate::core::internal_player::host_hwnd().is_none() {
            if let Some(h) = crate::core::internal_player::find_main_window() {
                crate::core::internal_player::set_host_hwnd(h);
            }
        }

        let Some(p) = self.internal.as_ref() else { return };

        // 渲染 API 初始化/出帧失败时，直接告诉用户原因（否则只会看到一个黑屏）
        if self.player_overlay.error.is_none() {
            if let Some(err) = p.render.error() {
                self.player_overlay.error = Some(err);
                self.player_overlay.status.clear();
            }
        }

        // 事件：进入播放 / 播放结束 / 出错
        for ev in p.poll_events() {
            match ev {
                crate::core::internal_player::MpvEvent::Playing => {
                    self.player_overlay.status.clear();
                }
                crate::core::internal_player::MpvEvent::Ended => {
                    if self.player_overlay.error.is_none() {
                        self.player_overlay.status = "播放已结束".into();
                    }
                }
                crate::core::internal_player::MpvEvent::Failed(msg) => {
                    crate::core::log::warn(format!("内置播放器错误: {msg}"));
                    self.player_overlay.error = Some(format!("播放失败: {msg}"));
                    self.player_overlay.status.clear();
                }
                // libmpv 方案暂不通过 Property 事件同步，直接轮询 get_property
            }
        }

        if !p.is_alive() {
            crate::core::log::warn("内置播放器进程已退出");
            self.internal = None;
            return;
        }

        // 进度与画面比例：不需要每帧都问 mpv，隔一段时间同步一次即可
        let now = std::time::Instant::now();
        if self
            .player_overlay
            .last_poll
            .map(|t| now.duration_since(t).as_millis() >= 500)
            .unwrap_or(true)
        {
            let p = self.internal.as_ref().unwrap();
            self.player_overlay.position = p.position_secs().unwrap_or(self.player_overlay.position);
            let dur = p.duration_secs().unwrap_or(0.0);
            if dur > 0.0 {
                self.player_overlay.duration = dur;
            }
            if self.player_overlay.aspect.is_none() {
                self.player_overlay.aspect = p
                    .video_size()
                    .map(|(w, h)| w as f32 / h as f32)
                    .filter(|r| *r > 0.05 && r.is_finite());
            }
            // 读不到就不动，避免属性偶发读取失败时绿条突然归零闪烁
            if let Some(cache) = p.cache_progress() {
                self.player_overlay.cache_progress = cache;
            }
            self.player_overlay.last_poll = Some(now);
        }
    }

    /// 绘制播放层（占满窗口）
    pub fn ui_player_overlay(&mut self, ctx: &egui::Context) {
        let fullscreen = self.player_overlay.fullscreen;

        // 全屏时控制条与进度条默认隐藏，鼠标移到窗口底部区域才浮出（沉浸式播放）
        // 渲染 API 下视频就是 egui 自己的矩形，鼠标事件能正常收到，hover 检测可靠
        let bottom_zone = 120.0;
        let pointer_near_bottom = ctx
            .input(|i| i.pointer.hover_pos())
            .map(|p| p.y > ctx.screen_rect().height() - bottom_zone)
            .unwrap_or(false);
        let show_bars = if fullscreen { pointer_near_bottom } else { true };

        // 非全屏：控制条用独立面板，不压画面
        if !fullscreen {
            egui::TopBottomPanel::bottom("internal_player_bar")
                .exact_height(46.0)
                .frame(egui::Frame::new().fill(BG_PANEL).inner_margin(egui::Margin::symmetric(8, 4)))
                .show(ctx, |ui| self.player_controls_buttons(ui));
        }

        // 进度条高度：非全屏时在中央面板底部预留；全屏时改为浮层，不改变视频尺寸
        let track_h = if fullscreen { 0.0 } else { 28.0 };

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();

                // 顶部信息条：标题 + 时间（一直显示）
                let info_h = 34.0;
                if info_h > 0.0 {
                    let info_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), info_h));
                    ui.painter().rect_filled(info_rect, 0.0, Color32::from_black_alpha(190));
                    let mut info_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(info_rect.shrink2(egui::vec2(10.0, 6.0))),
                    );
                    info_ui.horizontal(|ui| {
                        let label = if self.player_overlay.title.is_empty() {
                            "内置播放器".to_string()
                        } else {
                            self.player_overlay.title.clone()
                        };
                        ui.label(RichText::new(label).color(TEXT).size(13.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} / {}",
                                    fmt_time(self.player_overlay.position),
                                    if self.player_overlay.duration > 0.0 {
                                        fmt_time(self.player_overlay.duration)
                                    } else {
                                        "--:--".into()
                                    }
                                ))
                                .color(TEXT_DIM)
                                .size(12.0),
                            );
                        });
                    });
                }

                // 视频区：顶部信息条以下，进度条以上
                let video_area = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x, rect.min.y + info_h),
                    egui::pos2(rect.max.x, rect.max.y - track_h),
                );

                // 先挂上 mpv 渲染回调（画面在最底层），再画状态文字/按钮，保证文字盖在视频上
                self.draw_video(ui, ctx, video_area);

                // 双击视频：全屏时退出全屏，否则关闭播放层
                // （渲染 API 下视频区就是 egui 自己的矩形，鼠标事件可以正常收到）
                let video_interact = ui.interact(
                    video_area,
                    egui::Id::new("video_double_click"),
                    egui::Sense::click(),
                );
                if video_interact.double_clicked() {
                    // 双击 = 切换全屏（之前误写成“非全屏则关闭播放”，会被当成出错）
                    self.toggle_fullscreen(ctx);
                    return;
                }

                // 状态提示（缓冲中 / 错误）
                let mut message: Option<(String, Color32)> = None;
                if let Some(err) = &self.player_overlay.error {
                    message = Some((err.clone(), DANGER));
                } else if !self.player_overlay.status.is_empty() {
                    message = Some((self.player_overlay.status.clone(), TEXT_DIM));
                }
                if let Some((text, color)) = message {
                    ui.painter().rect_filled(video_area, 0.0, BG);
                    ui.painter().text(
                        video_area.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(15.0),
                        color,
                    );
                }

                if fullscreen {
                    // 全屏：进度条 + 按钮作为半透明浮层盖在画面上，隐藏时视频保持满屏
                    if show_bars {
                        let ov_h = 84.0;
                        let ov_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.min.x, rect.max.y - ov_h),
                            rect.max,
                        );
                        ui.painter()
                            .rect_filled(ov_rect, 0.0, Color32::from_black_alpha(200));

                        // 进度条在浮层上部
                        let track_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.min.x, rect.max.y - ov_h),
                            egui::pos2(rect.max.x, rect.max.y - ov_h + 28.0),
                        );
                        self.draw_progress_bar(ui, track_rect);

                        // 按钮行在浮层下部
                        let btn_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.min.x, rect.max.y - 50.0),
                            rect.max,
                        );
                        let mut btn_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(btn_rect.shrink2(egui::vec2(10.0, 3.0))),
                        );
                        self.player_controls_buttons(&mut btn_ui);
                    }
                } else {
                    // 进度条：撑满整个窗口宽度，在中央面板底部
                    let track_rect = egui::Rect::from_min_max(
                        egui::pos2(rect.min.x, rect.max.y - track_h),
                        rect.max,
                    );
                    self.draw_progress_bar(ui, track_rect);
                }
            });
    }

    /// 绘制进度条（播放进度 + 缓存进度，完全手动画，不用 Slider）
    fn draw_progress_bar(&mut self, ui: &mut egui::Ui, track_rect: egui::Rect) {
        let dur = self.player_overlay.duration.max(0.001);
        let pos = self.player_overlay.position.clamp(0.0, dur);

        // 整个 track_rect 都是可点击/拖拽区域
        let resp = ui.interact(track_rect, egui::Id::new("progress_bar"), egui::Sense::click_and_drag());

        // 底层：深灰背景（撑满 track_rect 宽度）
        ui.painter().rect_filled(track_rect, 0.0, Color32::from_rgb(40, 40, 40));

        // 中层：绿色缓存条（起点 0，终点 = 已缓存到的位置）
        // 保证不小于播放进度，否则蓝条已过、绿条没跟上，中间会出现断层
        let play_ratio = (pos / dur).clamp(0.0, 1.0);
        let cache = self.player_overlay.cache_progress.clamp(0.0, 1.0).max(play_ratio);
        if cache > 0.0 {
            let cache_w = track_rect.width() * cache as f32;
            let cache_rect = egui::Rect::from_min_max(
                track_rect.min,
                egui::pos2(track_rect.min.x + cache_w, track_rect.max.y),
            );
            ui.painter().rect_filled(cache_rect, 0.0, Color32::from_rgb(80, 200, 80));
        }

        // 上层：蓝色播放进度条（与背景同起点、同高、同宽度容器）
        if play_ratio > 0.0 {
            let play_w = track_rect.width() * play_ratio as f32;
            let play_rect = egui::Rect::from_min_max(
                track_rect.min,
                egui::pos2(track_rect.min.x + play_w, track_rect.max.y),
            );
            ui.painter().rect_filled(play_rect, 0.0, Color32::from_rgb(30, 144, 255));
        }

        // 鼠标悬停时显示白色指示线 + 时间
        if let Some(hover_pos) = resp.hover_pos() {
            let x = hover_pos.x.clamp(track_rect.min.x, track_rect.max.x);
            ui.painter().vline(x, track_rect.y_range(), egui::Stroke::new(1.5, Color32::WHITE));

            // 计算悬停位置对应的时间并显示
            let ratio = ((x - track_rect.min.x) / track_rect.width()).clamp(0.0, 1.0);
            let hover_time = dur * ratio as f64;
            let time_text = fmt_time(hover_time);

            // 在指示线上方显示时间（避免超出屏幕边界）
            let text_pos = egui::pos2(
                x,
                track_rect.min.y - 8.0,
            );
            ui.painter().text(
                text_pos,
                egui::Align2::CENTER_BOTTOM,
                &time_text,
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            );
        }

        // 点击/拖拽 seek
        if resp.drag_stopped() || resp.clicked() || resp.dragged() {
            if let Some(click_pos) = resp.interact_pointer_pos() {
                let ratio = ((click_pos.x - track_rect.min.x) / track_rect.width()).clamp(0.0, 1.0);
                let seek_to = dur * ratio as f64;
                if let Some(p) = self.internal.as_ref() {
                    p.seek_absolute(seek_to);
                }
                self.player_overlay.position = seek_to;
            }
        }
    }

    /// 把 mpv 渲染上下文挂到视频区上（由 egui 的 paint callback 在 GL 线程里真正出帧）
    ///
    /// 优点：视频画面与 egui 共用同一个窗口，全屏/缩放/叠加控件全部由 egui 自己掌控，
    /// 不会再有子窗口丢输入、全屏丢画面的问题。
    fn draw_video(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, area: egui::Rect) {
        let Some(render) = self.internal.as_ref().map(|p| p.render.clone()) else {
            return;
        };
        render.set_repaint_ctx(ctx.clone());

        let cb = egui::PaintCallback {
            rect: area,
            callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                move |info, painter| {
                    // 视频画到离屏 FBO，再按像素矩形贴回窗口（位置由 egui 布局决定）
                    let vp = info.viewport_in_pixels();
                    render.paint(
                        painter.gl(),
                        vp.left_px,
                        vp.from_bottom_px,
                        vp.width_px,
                        vp.height_px,
                    );
                },
            )),
        };
        ui.painter().add(cb);
    }

    /// 底部按钮行（按钮+时间+音量+全屏）
    fn player_controls_buttons(&mut self, ui: &mut egui::Ui) {
        let has_error = self.player_overlay.error.is_some();
        let dur = self.player_overlay.duration.max(0.001);
        let pos = self.player_overlay.position.clamp(0.0, dur);

        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("← 返回").color(ACCENT)).fill(CARD))
                .clicked()
            {
                self.stop_internal_player();
                return;
            }

            if ui
                .add(egui::Button::new(RichText::new("⏪ 10s").color(TEXT)).fill(CARD))
                .clicked()
            {
                self.seek_internal_relative(-10.0);
            }

            let play_label = if has_error {
                "↻ 重试"
            } else if self.player_overlay.paused {
                "▶ 播放"
            } else {
                "⏸ 暂停"
            };
            if ui
                .add(egui::Button::new(RichText::new(play_label).color(ACCENT)).fill(CARD))
                .clicked()
            {
                if has_error {
                    self.restart_internal_player();
                } else {
                    self.toggle_internal_pause();
                }
            }

            if ui
                .add(egui::Button::new(RichText::new("10s ⏩").color(TEXT)).fill(CARD))
                .clicked()
            {
                self.seek_internal_relative(10.0);
            }

            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    fmt_time(pos),
                    if self.player_overlay.duration > 0.0 {
                        fmt_time(self.player_overlay.duration)
                    } else {
                        "--:--".into()
                    }
                ))
                .color(TEXT_DIM)
                .size(12.0),
            );

            // 水平音量滑块
            ui.label(RichText::new("🔊").color(TEXT_DIM).size(14.0));
            let mut vol = self.player_overlay.volume as f32;
            if ui
                .add(egui::Slider::new(&mut vol, 0.0..=100.0).show_value(false))
                .changed()
            {
                let v = vol.round().clamp(0.0, 100.0) as u8;
                self.player_overlay.volume = v;
                self.settings.internal_volume = v;
                if let Some(p) = self.internal.as_ref() {
                    p.set_volume(v);
                }
            }

            // 全屏切换
            let fs_label = if self.player_overlay.fullscreen { "⛶ 退出" } else { "⛶ 全屏" };
            if ui
                .add(egui::Button::new(RichText::new(fs_label).color(ACCENT)).fill(CARD))
                .clicked()
            {
                self.toggle_fullscreen(ui.ctx());
            }
        });
    }

    /// 切换全屏：交给 winit 异步处理
    /// 不能在这里直接调 Win32 SetWindowLongPtr/SetWindowPos —— 那是在
    /// egui update 回调（winit 事件循环内部）里同步触发 WM_SIZE，
    /// 会导致窗口过程重入、与 mpv 渲染线程互相等锁 → 界面卡死无响应。
    pub fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        let target = !self.player_overlay.fullscreen;
        self.player_overlay.fullscreen = target;
        // 强制下一帧重新摆放 mpv 子窗口
        self.player_overlay.last_rect = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(target));
        ctx.request_repaint();
    }
}

/// 秒 → mm:ss / h:mm:ss
pub fn fmt_time(sec: f64) -> String {
    if !sec.is_finite() || sec <= 0.0 {
        return "00:00".into();
    }
    let total = sec.round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_time;

    #[test]
    fn formats_times() {
        assert_eq!(fmt_time(0.0), "00:00");
        assert_eq!(fmt_time(59.4), "00:59");
        assert_eq!(fmt_time(61.0), "01:01");
        assert_eq!(fmt_time(3661.0), "1:01:01");
        assert_eq!(fmt_time(f64::NAN), "00:00");
    }
}
