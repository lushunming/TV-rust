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
                ui.heading(RichText::new("外部播放器").color(TEXT));
                ui.label(RichText::new("按 exe 名称自动匹配参数：potplayer / vlc / mpc-be(hc) / mpv，其他播放器仅传 URL。").color(TEXT_DIM).size(12.0));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("路径:");
                    let before = self.settings_page.player_path_edit.clone();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_page.player_path_edit)
                            .hint_text("例如 C:\\Program Files\\VideoLAN\\VLC\\vlc.exe")
                            .desired_width(ui.available_width() - 160.0),
                    );
                    if self.settings_page.player_path_edit != before {
                        self.settings_page.dirty = true;
                    }
                    if ui.button("检测").clicked() {
                        self.settings_page.detected = crate::core::settings::detect_players();
                    }
                });
                for d in &self.settings_page.detected {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("[{}] {}", d.kind, d.path)).color(TEXT_DIM).size(12.0));
                        if ui.button(RichText::new("使用").size(12.0).color(ACCENT)).clicked() {
                            self.settings_page.player_path_edit = d.path.clone();
                            self.settings_page.dirty = true;
                        }
                    });
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
