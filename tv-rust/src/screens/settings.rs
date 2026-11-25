//! 设置页：配置源管理 + 播放器配置 + 宿主(spider-host)配置

use crate::app::TVApp;
use crate::theme::{ACCENT, TEXT, TEXT_DIM};
use egui::{RichText, ScrollArea};

#[derive(Default)]
pub struct SettingsPageState {
    pub new_source: String,
    pub player_path_edit: String,
    pub java_path_edit: String,
    pub host_jar_edit: String,
    pub detected: Vec<crate::core::settings::DetectedPlayer>,
    pub dirty: bool,
}

impl TVApp {
    pub fn ui_settings(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.add_space(6.0);

                // ---------- 配置源 ----------
                ui.heading(RichText::new("站点配置").color(TEXT));
                ui.label(
                    RichText::new("支持 http(s) URL、本地文件路径或直接粘贴 JSON 配置（FongMi 格式）。配置切换后自动加载。")
                        .color(TEXT_DIM).size(12.0),
                );
                ui.add_space(4.0);
                let sources = self.settings.config_sources.clone();
                for (i, src) in sources.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let active = self.settings.active_source == Some(i);
                        if ui.radio(active, trunc(src, 70)).clicked() {
                            self.settings.active_source = Some(i);
                            self.settings.save();
                            self.request_load_config();
                        }
                        if !active {
                            if ui.button(RichText::new("使用").size(12.0).color(ACCENT)).clicked() {
                                self.settings.active_source = Some(i);
                                self.settings.save();
                                self.request_load_config();
                            }
                        }
                        if ui.button(RichText::new("删除").size(12.0).color(TEXT_DIM)).clicked() {
                            self.settings.config_sources.remove(i);
                            if self.settings.active_source == Some(i) {
                                self.settings.active_source = None;
                            }
                            self.settings.save();
                            self.toast("已删除配置源");
                        }
                    });
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.settings_page.new_source)
                            .hint_text("输入配置 URL / 文件路径 / JSON")
                            .desired_width(ui.available_width() - 70.0),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button(RichText::new("添加并加载").color(ACCENT)).clicked() || enter {
                        let s = self.settings_page.new_source.trim().to_string();
                        if !s.is_empty() {
                            self.settings.config_sources.push(s);
                            let idx = self.settings.config_sources.len() - 1;
                            self.settings.active_source = Some(idx);
                            self.settings.save();
                            self.settings_page.new_source.clear();
                            self.request_load_config();
                        }
                    }
                });

                ui.add_space(14.0);
                ui.separator();

                // ---------- 播放器 ----------
                ui.heading(RichText::new("播放器").color(TEXT));
                ui.label(
                    RichText::new(
                        "默认使用内置播放器（libmpv 原生渲染，窗口内播放，无需额外配置）。\n\
                        若您偏好 vlc、mpv、MPC-HC 等外部播放器，可在下方填写路径；\n\
                        填写后播放将优先使用外部播放器，留空则继续使用内置播放器。",
                    )
                    .color(TEXT_DIM)
                    .size(12.0),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("外部播放器路径:").color(TEXT_DIM));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.settings_page.player_path_edit)
                            .hint_text("留空则使用内置播放器")
                            .desired_width(ui.available_width() - 260.0),
                    );
                    if resp.changed() || resp.lost_focus() {
                        self.settings.player_path = self.settings_page.player_path_edit.trim().to_string();
                        self.settings.save();
                    }
                    if ui.button("自动探测").clicked() {
                        self.settings_page.detected = crate::core::settings::detect_players();
                        if let Some(first) = self.settings_page.detected.first() {
                            self.settings_page.player_path_edit = first.path.clone();
                            self.settings.player_path = first.path.clone();
                            self.settings.save();
                            self.toast(format!("已找到外部播放器: {} ({})", first.kind, first.path));
                        } else {
                            self.toast("未找到外部播放器，请手动填写路径");
                        }
                    }
                });
                if !self.settings_page.detected.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("探测到的播放器:").color(TEXT_DIM).size(11.5));
                        for d in &self.settings_page.detected {
                            if ui
                                .selectable_label(
                                    self.settings_page.player_path_edit == d.path,
                                    RichText::new(format!("{} ({})", d.kind, trunc(&d.path, 40)))
                                        .color(TEXT_DIM)
                                        .size(11.5),
                                )
                                .clicked()
                            {
                                self.settings_page.player_path_edit = d.path.clone();
                                self.settings.player_path = d.path.clone();
                                self.settings.save();
                            }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    let hint = if self.settings.player_path.trim().is_empty() {
                        "当前使用内置播放器（libmpv）".to_string()
                    } else {
                        format!("当前将使用外部播放器: {}", self.settings.player_path)
                    };
                    ui.label(RichText::new(hint).color(TEXT_DIM).size(11.5));
                });

                // ---------- 内置播放器组件（libmpv 按需下载）----------
                // 包不再肿 118 MB；首次使用点一下即可从上游官方构建下载并解包。
                ui.add_space(8.0);
                use crate::core::libmpv_install::InstallState;
                let dl_state = self.libmpv_installer.state();

                ui.horizontal(|ui| {
                    ui.label(RichText::new("内置播放器组件:").color(TEXT_DIM));
                    match crate::core::settings::detect_libmpv() {
                        Some(p) => {
                            ui.label(RichText::new("已就绪").color(ACCENT).size(11.5));
                            ui.label(RichText::new(trunc(&p, 52)).color(TEXT_DIM).size(11.0));
                        }
                        None => {
                            ui.label(
                                RichText::new("未安装（首次使用需下载，约 33 MB）")
                                    .color(TEXT_DIM)
                                    .size(11.5),
                            );
                        }
                    }
                });

                match dl_state.clone() {
                    InstallState::Downloading { got, total } => {
                        let frac = if total > 0 {
                            (got as f32 / total as f32).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::ProgressBar::new(frac).desired_width(300.0).text(format!(
                                    "{:.1} / {:.1} MB",
                                    got as f64 / 1e6,
                                    total as f64 / 1e6
                                )),
                            );
                            if ui.button("取消").clicked() {
                                self.libmpv_installer.cancel();
                            }
                        });
                    }
                    InstallState::Resolving | InstallState::Extracting => {
                        let text = if matches!(dl_state, InstallState::Resolving) {
                            "正在获取下载地址…"
                        } else {
                            "正在解包（约 118 MB，稍等）…"
                        };
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(text).color(TEXT_DIM).size(11.5));
                        });
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            #[cfg(windows)]
                            let btn = ui.button("下载内置播放器组件（约 33 MB）");
                            #[cfg(not(windows))]
                            let btn = ui.button("查看安装方法");
                            if btn.clicked() {
                                let ctx = ui.ctx().clone();
                                self.libmpv_installer.start(&ctx);
                            }
                            if matches!(dl_state, InstallState::Cancelled) {
                                ui.label(RichText::new("已取消").color(TEXT_DIM).size(11.5));
                            }
                        });
                    }
                }

                match &dl_state {
                    InstallState::Done(p) => {
                        ui.label(
                            RichText::new(format!("安装完成: {}", trunc(p, 60)))
                                .color(ACCENT)
                                .size(11.0),
                        );
                    }
                    InstallState::Failed(e) => {
                        ui.label(
                            RichText::new(format!("下载失败: {}", trunc(e, 100)))
                                .color(egui::Color32::from_rgb(220, 90, 90))
                                .size(11.0),
                        );
                        ui.label(
                            RichText::new("可稍后重试；或手动下载 mpv-dev-x86_64-*.7z 解压后把 mpv-2.dll 放到 exe 同级目录")
                                .color(TEXT_DIM)
                                .size(11.0),
                        );
                    }
                    _ => {}
                }

                ui.add_space(14.0);
                ui.separator();

                // ---------- 宿主 ----------
                ui.heading(RichText::new("spider-host 宿主（Java 爬虫）").color(TEXT));
                ui.label(
                    RichText::new("Rust UI 通过子进程 spider-host.jar 调用 Java 爬虫。jar 由 gradlew :spider-host:fatJar 构建。")
                        .color(TEXT_DIM).size(12.0),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("java:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_page.java_path_edit)
                            .hint_text("java")
                            .desired_width(ui.available_width() - 260.0),
                    );
                    ui.label("jar:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_page.host_jar_edit)
                            .hint_text("spider-host-all.jar 路径")
                            .desired_width(ui.available_width() - 160.0),
                    );
                    if ui.button("自动探测").clicked() {
                        if let Some(p) = crate::core::settings::autodetect_host_jar() {
                            self.settings_page.host_jar_edit = p.to_string_lossy().to_string();
                            self.settings_page.dirty = true;
                        } else {
                            self.toast("未找到 spider-host-all.jar，请手动填写");
                        }
                    }
                });
                ui.horizontal(|ui| {
                    match &self.host_status {
                        crate::app::HostStatus::Starting => ui.label(RichText::new("宿主: 启动中…").color(TEXT_DIM)),
                        crate::app::HostStatus::Ready(info) => ui.label(RichText::new(format!("宿主: {}", info)).color(ACCENT)),
                        crate::app::HostStatus::Failed(e) => ui.label(RichText::new(format!("宿主: {}", e)).color(crate::theme::DANGER)),
                    };
                    if ui.button("重启宿主").clicked() {
                        self.restart_host();
                    }
                });

                ui.add_space(14.0);
                ui.separator();

                // ---------- 诊断日志 ----------
                ui.heading(RichText::new("诊断日志").color(TEXT));
                ui.label(
                    RichText::new("遇到问题时先把日志发出来：程序日志、宿主日志、播放器输出都在缓存目录下。")
                        .color(TEXT_DIM).size(12.0),
                );
                let log_dir = crate::core::paths::rust_data();
                ui.label(
                    RichText::new(format!("目录: {}", log_dir.display()))
                        .color(TEXT_DIM).size(11.0),
                );
                ui.horizontal(|ui| {
                    if ui.button("打开日志目录").clicked() {
                        open_path(&log_dir);
                    }
                    let player_log = crate::core::paths::player_log();
                    if ui.button("打开 playerLog").clicked() {
                        open_path(&player_log);
                    }
                });

                ui.add_space(14.0);
                ui.separator();

                // ---------- 保存 ----------
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("保存设置").color(ACCENT)).clicked() {
                        self.apply_settings_edits();
                    }
                    if self.settings_page.dirty {
                        ui.label(RichText::new("有未保存的修改").color(TEXT_DIM));
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

/// 在资源管理器中打开目录（或文件的默认打开方式）
fn open_path(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}
