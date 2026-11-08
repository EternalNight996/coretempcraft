#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cpu;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Layout, Margin, RichText,
    ScrollArea, Sense, Stroke, StrokeKind, TextStyle, Vec2,
};

/// Radius values are written as floats in this app; egui 0.36 stores them as whole pixels.
struct Rounding;

impl Rounding {
    fn same(radius: f32) -> CornerRadius {
        CornerRadius::same(radius.round() as u8)
    }
    const ZERO: CornerRadius = CornerRadius::ZERO;
}

trait FrameRoundingExt {
    fn rounding(self, radius: impl Into<CornerRadius>) -> Self;
}

impl FrameRoundingExt for egui::Frame {
    fn rounding(self, radius: impl Into<CornerRadius>) -> Self {
        self.corner_radius(radius)
    }
}

trait ButtonRoundingExt {
    fn rounding(self, radius: impl Into<CornerRadius>) -> Self;
}

impl ButtonRoundingExt for egui::Button<'_> {
    fn rounding(self, radius: impl Into<CornerRadius>) -> Self {
        self.corner_radius(radius)
    }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

const BG: Color32 = Color32::from_rgb(8, 11, 16);
const BG_TOP: Color32 = Color32::from_rgb(10, 14, 20);
const CARD: Color32 = Color32::from_rgb(17, 23, 31);
const CARD_HI: Color32 = Color32::from_rgb(22, 30, 40);
const CODE_BG: Color32 = Color32::from_rgb(11, 15, 21);
const BORDER: Color32 = Color32::from_rgb(34, 45, 58);
const TEXT: Color32 = Color32::from_rgb(233, 241, 249);
const MUTED: Color32 = Color32::from_rgb(141, 156, 173);
const DIM: Color32 = Color32::from_rgb(92, 106, 122);
const ACCENT: Color32 = Color32::from_rgb(62, 231, 195);
const BLUE: Color32 = Color32::from_rgb(122, 162, 255);
const GOLD: Color32 = Color32::from_rgb(255, 197, 92);
const CORAL: Color32 = Color32::from_rgb(255, 119, 105);
const CODE_KEY: Color32 = Color32::from_rgb(139, 190, 255);
const CODE_COMMENT: Color32 = Color32::from_rgb(104, 122, 140);
const CODE_TEXT: Color32 = Color32::from_rgb(205, 218, 232);
const CODE_NUM: Color32 = Color32::from_rgb(66, 79, 94);

const HISTORY_LEN: usize = 56;
const WARMUP_SAMPLES: usize = 44;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Monitor,
    Api,
}

struct App {
    sampler: cpu::CpuSampler,
    snapshot: cpu::CpuSnapshot,
    history: Vec<VecDeque<f32>>,
    last_sample: Instant,
    interval_ms: u64,
    paused: bool,
    tab: Tab,
    frames: u64,
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    toast: Option<(String, Instant)>,
    api_code: &'static str,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<PathBuf>) -> Self {
        install_fonts(&cc.egui_ctx);
        install_style(&cc.egui_ctx);

        let mut sampler = cpu::CpuSampler::new();
        let snapshot = sampler.sample();
        let mut history = vec![VecDeque::new(); snapshot.cores.len()];
        for core in &snapshot.cores {
            if let Some(h) = history.get_mut(core.index) {
                h.push_back(core.current_mhz);
            }
        }

        Self {
            sampler,
            snapshot,
            history,
            last_sample: Instant::now(),
            interval_ms: cpu::DEFAULT_INTERVAL_MS,
            paused: false,
            tab: Tab::Monitor,
            frames: 0,
            screenshot,
            screenshot_requested: false,
            toast: None,
            api_code: include_str!("cpu.rs"),
        }
    }

    fn warmup(&self) -> bool {
        self.history
            .first()
            .map(|h| h.len() < WARMUP_SAMPLES)
            .unwrap_or(false)
    }

    fn tick(&mut self) {
        self.snapshot = self.sampler.sample();
        if self.history.len() != self.snapshot.cores.len() {
            self.history
                .resize_with(self.snapshot.cores.len(), VecDeque::new);
        }
        for core in &self.snapshot.cores {
            if let Some(h) = self.history.get_mut(core.index) {
                h.push_back(core.current_mhz);
                while h.len() > HISTORY_LEN {
                    h.pop_front();
                }
            }
        }
        self.last_sample = Instant::now();
    }

    fn sample_now(&mut self) {
        self.tick();
    }

    fn copy(&mut self, ctx: &egui::Context, text: &str, what: &str) {
        ctx.copy_text(text.to_owned());
        self.toast = Some((format!("{what}已复制到剪贴板"), Instant::now()));
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else {
            return;
        };
        if !self.screenshot_requested && self.frames >= 120 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.screenshot_requested = true;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            match save_screenshot(&path, &image) {
                Ok(()) => std::process::exit(0),
                Err(err) => {
                    let err_path = path.with_extension("error.txt");
                    let _ = std::fs::write(err_path, err);
                    std::process::exit(1);
                }
            }
        }
        ctx.request_repaint();
    }

    fn draw(&mut self, root: &mut egui::Ui, ctx: &egui::Context) {
        paint_background(ctx);
        self.top_bar(root);

        egui::CentralPanel::default_margins()
            .frame(
                egui::Frame::default()
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(Margin::symmetric(28, 10)),
            )
            .show(root, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt("page_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.tab {
                            Tab::Monitor => self.draw_monitor(ui),
                            Tab::Api => self.draw_api(ui),
                        }
                    });
            });

        self.draw_toast(ctx);
    }

    fn top_bar(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("top_bar")
            .exact_size(74.0)
            .frame(
                egui::Frame::default()
                    .fill(BG_TOP)
                    .inner_margin(Margin::symmetric(28, 0)),
            )
            .show(root, |ui| {
                let rect = ui.max_rect();
                ui.painter()
                    .hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, BORDER));
                ui.allocate_ui_with_layout(
                    Vec2::new(rect.width(), 74.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                    let (icon_rect, _) =
                        ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                    paint_chip(ui.painter(), icon_rect.shrink(3.0), ACCENT);
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.add_space(15.0);
                        ui.label(
                            RichText::new("主频监视器")
                                .size(17.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new("CPU FREQUENCY  ·  RUST")
                                .size(9.0)
                                .color(DIM),
                        );
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.live_pill(ui);
                        ui.add_space(16.0);
                        if tab_button(ui, "Rust 接口", self.tab == Tab::Api) {
                            self.tab = Tab::Api;
                        }
                        if tab_button(ui, "监视器", self.tab == Tab::Monitor) {
                            self.tab = Tab::Monitor;
                        }
                    });
                    },
                );
            });
    }

    fn live_pill(&self, ui: &mut egui::Ui) {
        let live = self.snapshot.source_ok && !self.paused;
        let (label, color) = if live {
            ("实时", ACCENT)
        } else if self.paused {
            ("已暂停", GOLD)
        } else {
            ("不可用", CORAL)
        };
        egui::Frame::default()
            .fill(Color32::from_rgb(15, 22, 29))
            .stroke(Stroke::new(1.0, BORDER))
            .rounding(Rounding::same(20.0))
            .inner_margin(Margin::symmetric(11, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.0, color);
                    ui.label(RichText::new(label).size(11.0).color(color));
                });
            });
    }

    fn draw_monitor(&mut self, ui: &mut egui::Ui) {
        if !self.snapshot.source_ok {
            warning_banner(
                ui,
                "无法读取实时主频：Windows 电源管理接口没有返回数据。下方数值仅作占位。",
            );
            ui.add_space(10.0);
        }

        self.processor_heading(ui);
        ui.add_space(12.0);
        self.hero(ui);
        ui.add_space(18.0);
        self.cores_header(ui);
        ui.add_space(10.0);
        self.core_grid(ui);
        ui.add_space(14.0);
        self.footer(ui);
    }

    fn processor_heading(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("处理器").size(10.0).color(DIM));
                ui.label(
                    RichText::new(self.sampler.cpu_name())
                        .size(19.0)
                        .strong()
                        .color(TEXT),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                pill(
                    ui,
                    &format!("{} 逻辑核心", self.snapshot.cores.len()),
                    BLUE,
                );
                ui.add_space(8.0);
                pill(
                    ui,
                    &format!("{}", short_vendor(self.sampler.vendor())),
                    DIM,
                );
            });
        });
    }

    fn hero(&mut self, ui: &mut egui::Ui) {
        let peak_core = self
            .snapshot
            .cores
            .iter()
            .max_by(|a, b| a.current_mhz.total_cmp(&b.current_mhz))
            .cloned()
            .unwrap_or_default();
        let animated = ui.ctx().animate_value_with_time(
            Id::new("hero_peak"),
            peak_core.current_mhz,
            0.35,
        );

        egui::Frame::default()
            .fill(CARD)
            .stroke(Stroke::new(1.0, BORDER))
            .rounding(Rounding::same(20.0))
            .inner_margin(Margin::same(24))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    let total = ui.available_width();
                    let left_w = (total * 0.57).max(430.0);
                    let right_w = (total - left_w - 16.0).max(300.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_w, 166.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(left_w);
                            ui.horizontal(|ui| {
                                let (dot, _) =
                                    ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                                ui.painter().circle_filled(dot.center(), 4.0, ACCENT);
                                ui.label(
                                    RichText::new("最高核心频率")
                                        .size(11.5)
                                        .color(MUTED),
                                );
                                ui.label(
                                    RichText::new(format!("核心 {:02}", peak_core.index))
                                        .size(10.0)
                                        .color(DIM),
                                );
                            });
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                let text = if peak_core.current_mhz > 0.5 {
                                    thousands(animated)
                                } else {
                                    "—".to_string()
                                };
                                ui.label(
                                    RichText::new(text)
                                        .font(FontId::new(58.0, FontFamily::Monospace))
                                        .color(TEXT),
                                );
                                ui.add_space(2.0);
                                ui.label(RichText::new("MHz").size(14.0).color(MUTED));
                            });
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(format!(
                                    "× {:.2}   ·   基准 {:.2} MHz   ·   全核平均 {} MHz",
                                    peak_core.multiplier,
                                    self.snapshot.base_mhz,
                                    thousands(self.snapshot.avg_mhz)
                                ))
                                .size(12.0)
                                .color(MUTED),
                            );
                            ui.add_space(14.0);
                            ui.horizontal(|ui| {
                                pill(
                                    ui,
                                    &format!("负载 {:.0}%", peak_core.load * 100.0),
                                    load_color(peak_core.load),
                                );
                                pill(
                                    ui,
                                    &format!("{} 线程", self.snapshot.cores.len()),
                                    DIM,
                                );
                            });
                        },
                    );
                    ui.add_space(16.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(right_w, 166.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(right_w);
                            let block_w = (right_w - 10.0) / 2.0;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                stat_block(
                                    ui,
                                    block_w,
                                    "基准频率",
                                    format!("{:.2}", self.snapshot.base_mhz),
                                    "MHz",
                                    BLUE,
                                );
                                stat_block(
                                    ui,
                                    block_w,
                                    "当前倍频",
                                    format!("{:.2}", peak_core.multiplier),
                                    "×",
                                    ACCENT,
                                );
                            });
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                stat_block(
                                    ui,
                                    block_w,
                                    "最大加速",
                                    format!("{:.0}", self.snapshot.max_mhz),
                                    "MHz",
                                    GOLD,
                                );
                                stat_block(
                                    ui,
                                    block_w,
                                    "平均负载",
                                    format!("{:.0}", self.snapshot.avg_load * 100.0),
                                    "%",
                                    load_color(self.snapshot.avg_load),
                                );
                            });
                        },
                    );
                });
            });
    }

    fn cores_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("各核心实时频率")
                    .size(16.0)
                    .strong()
                    .color(TEXT),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{} 个逻辑核心", self.snapshot.cores.len()))
                    .size(11.0)
                    .color(DIM),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let pause_label = if self.paused { "继续" } else { "暂停" };
                if ghost_button(ui, pause_label) {
                    self.paused = !self.paused;
                    if !self.paused {
                        self.sample_now();
                    }
                }
                ui.add_space(10.0);
                egui::ComboBox::from_id_salt("interval_combo")
                    .width(92.0)
                    .selected_text(format!("{} ms", self.interval_ms))
                    .show_ui(ui, |ui| {
                        for value in [250_u64, 500, 1000, 2000] {
                            ui.selectable_value(
                                &mut self.interval_ms,
                                value,
                                format!("{value} ms"),
                            );
                        }
                    });
                ui.label(RichText::new("更新间隔").size(11.0).color(DIM));
            });
        });
    }

    fn core_grid(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_width();
        let gap = 12.0;
        let min_card = 224.0;
        let cols = (((avail + gap) / (min_card + gap)).floor() as usize).clamp(2, 4);
        let card_w = (avail - gap * (cols as f32 - 1.0)) / cols as f32;
        let display_max = self
            .snapshot
            .max_mhz
            .max(self.snapshot.peak_mhz)
            .max(1.0);
        let cores = self.snapshot.cores.clone();
        for chunk in cores.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for core in chunk {
                    let history = self
                        .history
                        .get(core.index)
                        .cloned()
                        .unwrap_or_default();
                    core_card(ui, card_w, core, &history, display_max);
                }
            });
            ui.add_space(gap);
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let age = self.last_sample.elapsed().as_secs_f32();
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(6.0), Sense::hover());
            ui.painter()
                .circle_filled(dot.center(), 3.0, if self.snapshot.source_ok { ACCENT } else { CORAL });
            ui.label(
                RichText::new(
                    "数据来源：Windows 电源管理 NtPowerInformation  ·  无需内核驱动",
                )
                .size(10.5)
                .color(DIM),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("上次采样 {:.1} 秒前", age))
                        .size(10.5)
                        .color(DIM),
                );
            });
        });
    }

    fn draw_api(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("Rust 接口").size(23.0).strong().color(TEXT));
                ui.label(
                    RichText::new(
                        "一个不依赖内核驱动的实时主频读取接口，已在本程序中运行。",
                    )
                    .size(12.0)
                    .color(MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let code = self.api_code.to_string();
                if primary_button(ui, "复制完整代码") {
                    self.copy(ui.ctx(), &code, "完整代码");
                }
                ui.add_space(8.0);
                if ghost_button(ui, "复制调用示例") {
                    self.copy(ui.ctx(), USAGE_SNIPPET, "调用示例");
                }
            });
        });
        ui.add_space(16.0);

        let total = ui.available_width();
        let left_w = 344.0_f32.min(total * 0.32);
        let gap = 16.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(left_w, 590.0),
                Layout::top_down(Align::Min),
                |ui| {
                    api_overview(ui);
                },
            );
            ui.add_space(gap);
            ui.allocate_ui_with_layout(
                Vec2::new((total - left_w - gap).max(360.0), 590.0),
                Layout::top_down(Align::Min),
                |ui| {
                    self.code_panel(ui);
                },
            );
        });

        ui.add_space(16.0);
        usage_panel(ui);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(
                    "提示：接口每调用一次就重新查询一遍全部逻辑核心；界面按 1 秒节奏调用，与原程序 ReadInt 默认值一致。",
                )
                .size(10.5)
                .color(DIM),
            );
        });
    }

    fn code_panel(&mut self, ui: &mut egui::Ui) {
        let line_count = self.api_code.lines().count();
        egui::Frame::default()
            .fill(CODE_BG)
            .stroke(Stroke::new(1.0, BORDER))
            .rounding(Rounding::same(14.0))
            .inner_margin(Margin::same(0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(590.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new("cpu.rs")
                            .font(FontId::new(12.0, FontFamily::Monospace))
                            .color(ACCENT),
                    );
                    ui.label(RichText::new(format!("{line_count} 行")).size(10.0).color(DIM));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(14.0);
                        if ghost_button(ui, "复制") {
                            let code = self.api_code.to_string();
                            self.copy(ui.ctx(), &code, "完整代码");
                        }
                    });
                });
                ui.add_space(6.0);
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.cursor().top(),
                    Stroke::new(1.0, BORDER),
                );
                ui.add_space(4.0);
                let code = self.api_code;
                ScrollArea::vertical()
                    .id_salt("code_scroll")
                    .auto_shrink([false, false])
                    .max_height(520.0)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        for (idx, line) in code.lines().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add_space(12.0);
                                ui.add_sized(
                                    [34.0, 15.0],
                                    egui::Label::new(
                                        RichText::new(format!("{:>3}", idx + 1))
                                            .font(FontId::new(10.0, FontFamily::Monospace))
                                            .color(CODE_NUM),
                                    ),
                                );
                                ui.label(
                                    RichText::new(line)
                                        .font(FontId::new(11.5, FontFamily::Monospace))
                                        .color(code_line_color(line)),
                                );
                            });
                        }
                        ui.add_space(10.0);
                    });
            });
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((message, at)) = self.toast.clone() else {
            return;
        };
        if at.elapsed() > Duration::from_secs(2) {
            self.toast = None;
            return;
        }
        egui::Area::new(Id::new("toast"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -22.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(Color32::from_rgb(20, 30, 38))
                    .stroke(Stroke::new(1.0, ACCENT.gamma_multiply(0.5)))
                    .rounding(Rounding::same(20.0))
                    .inner_margin(Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(message).size(12.0).color(TEXT));
                    });
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frames += 1;

        let warm = self.warmup();
        let due = self.last_sample.elapsed() >= Duration::from_millis(self.interval_ms);
        if !self.paused && (warm || due) {
            self.tick();
        }

        self.draw(ui, &ctx);

        if self.screenshot.is_some() {
            self.handle_screenshot(&ctx);
        } else {
            let next = if warm { 16 } else { 250 };
            ctx.request_repaint_after(Duration::from_millis(next));
        }
    }
}

// ---------------------------------------------------------------------------
// Page pieces
// ---------------------------------------------------------------------------

fn api_overview(ui: &mut egui::Ui) {
    egui::Frame::default()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(14.0))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("接口速览").size(14.0).strong().color(TEXT));
            ui.add_space(10.0);
            api_row(ui, "CpuSampler::new()", "初始化型号、基准频率与核心数");
            api_row(ui, "sampler.sample()", "返回一次全核快照");
            api_row(ui, "snapshot.peak_mhz", "当前最高核心频率");
            api_row(ui, "snapshot.avg_load", "全核平均负载 0..1");
            api_row(ui, "core.multiplier", "倍频 = 当前 / 基准");
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(10.0);
            ui.label(
                RichText::new("与原程序的关系")
                    .size(12.0)
                    .strong()
                    .color(TEXT),
            );
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "原程序用内核驱动读 MSR 0x198 的倍频，再乘实测基频；\
                         本接口改用 Windows 电源管理的实时 MHz，无需驱动和提权。\
                         两者都是有效频率，数值可能相差几十 MHz。",
                    )
                    .size(11.0)
                    .color(MUTED),
                )
                .wrap(),
            );
            ui.add_space(10.0);
            ui.label(
                RichText::new("采样间隔默认 1000 ms（同 ReadInt）")
                    .size(10.5)
                    .color(DIM),
            );
        });
}

fn api_row(ui: &mut egui::Ui, signature: &str, description: &str) {
    ui.label(
        RichText::new(signature)
            .font(FontId::new(11.5, FontFamily::Monospace))
            .color(ACCENT),
    );
    ui.label(RichText::new(description).size(10.5).color(MUTED));
    ui.add_space(6.0);
}

fn usage_panel(ui: &mut egui::Ui) {
    egui::Frame::default()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(14.0))
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("调用示例").size(13.0).strong().color(TEXT));
            ui.add_space(6.0);
            for line in USAGE_SNIPPET.lines() {
                ui.label(
                    RichText::new(line)
                        .font(FontId::new(11.0, FontFamily::Monospace))
                        .color(if line.trim_start().starts_with("//") {
                            CODE_COMMENT
                        } else {
                            CODE_TEXT
                        }),
                );
            }
        });
}

const USAGE_SNIPPET: &str = r#"// 放进任意 Rust 项目，复用 src/cpu.rs 即可
let mut sampler = cpu::CpuSampler::new();
loop {
    let snap = sampler.sample();
    for core in &snap.cores {
        println!("core {}: {:.0} MHz  x{:.1}  {:.0}%",
            core.index, core.current_mhz, core.multiplier, core.load * 100.0);
    }
    std::thread::sleep(std::time::Duration::from_millis(1000));
}"#;

// ---------------------------------------------------------------------------
// Small widgets
// ---------------------------------------------------------------------------

fn tab_button(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    let text = RichText::new(label)
        .size(12.5)
        .color(if active { TEXT } else { MUTED });
    let fill = if active {
        Color32::from_rgb(28, 39, 51)
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if active {
        Stroke::new(1.0, BORDER)
    } else {
        Stroke::NONE
    };
    ui.add(
        egui::Button::new(text)
            .fill(fill)
            .stroke(stroke)
            .rounding(Rounding::same(9.0))
            .min_size(Vec2::new(92.0, 32.0)),
    )
    .clicked()
}

fn ghost_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).size(11.5).color(MUTED))
            .fill(Color32::from_rgb(18, 25, 34))
            .stroke(Stroke::new(1.0, BORDER))
            .rounding(Rounding::same(9.0))
            .min_size(Vec2::new(64.0, 30.0)),
    )
    .clicked()
}

fn primary_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(label).size(11.5).color(BG).strong())
            .fill(ACCENT)
            .stroke(Stroke::NONE)
            .rounding(Rounding::same(9.0))
            .min_size(Vec2::new(112.0, 32.0)),
    )
    .clicked()
}

fn pill(ui: &mut egui::Ui, label: &str, color: Color32) {
    egui::Frame::default()
        .fill(Color32::from_rgb(15, 22, 29))
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(20.0))
        .inner_margin(Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(10.0).color(color));
        });
}

fn stat_block(
    ui: &mut egui::Ui,
    width: f32,
    label: &str,
    value: String,
    unit: &str,
    unit_color: Color32,
) {
    egui::Frame::default()
        .fill(CARD_HI)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width((width - 24.0).max(80.0));
            ui.set_min_height(52.0);
            ui.label(RichText::new(label).size(9.5).color(MUTED));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(value)
                        .font(FontId::new(19.0, FontFamily::Monospace))
                        .color(TEXT),
                );
                ui.label(RichText::new(unit).size(9.0).color(unit_color));
            });
        });
}

fn core_card(
    ui: &mut egui::Ui,
    width: f32,
    core: &cpu::CoreSample,
    history: &VecDeque<f32>,
    display_max: f32,
) {
    let ratio = (core.current_mhz / display_max).clamp(0.0, 1.0);
    let color = freq_color(ratio);
    let inner_w = (width - 28.0).max(120.0);
    egui::Frame::default()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(14.0))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(inner_w);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("核心 {:02}", core.index))
                        .size(10.5)
                        .color(MUTED),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{:.0}%", core.load * 100.0))
                            .size(10.0)
                            .color(load_color(core.load)),
                    );
                    let (dot, _) =
                        ui.allocate_exact_size(Vec2::splat(5.0), Sense::hover());
                    ui.painter()
                        .circle_filled(dot.center(), 2.5, load_color(core.load));
                });
            });
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{:.0}", core.current_mhz))
                        .font(FontId::new(26.0, FontFamily::Monospace))
                        .color(TEXT),
                );
                ui.add_space(1.0);
                ui.label(RichText::new("MHz").size(9.5).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("x{:.1}", core.multiplier))
                            .font(FontId::new(10.5, FontFamily::Monospace))
                            .color(color),
                    );
                });
            });
            ui.add_space(5.0);
            let (bar, _) = ui.allocate_exact_size(Vec2::new(inner_w, 4.0), Sense::hover());
            ui.painter()
                .rect_filled(bar, Rounding::same(2.0), Color32::from_rgb(28, 37, 48));
            let mut fill = bar;
            fill.set_width(bar.width() * ratio.max(0.015));
            ui.painter().rect_filled(fill, Rounding::same(2.0), color);

            ui.add_space(5.0);
            let (spark, _) =
                ui.allocate_exact_size(Vec2::new(inner_w, 24.0), Sense::hover());
            draw_history_bars(ui.painter(), spark, history, display_max, color);
        });
}

fn draw_history_bars(
    painter: &egui::Painter,
    rect: egui::Rect,
    history: &VecDeque<f32>,
    display_max: f32,
    color: Color32,
) {
    let n = history.len().min(HISTORY_LEN);
    if n == 0 || rect.width() <= 0.0 {
        return;
    }
    let bar_w = rect.width() / HISTORY_LEN as f32;
    let gap = (bar_w * 0.28).clamp(0.4, 1.2);
    for (k, value) in history.iter().enumerate().skip(history.len().saturating_sub(n)) {
        let idx = k - history.len().saturating_sub(n);
        let x = rect.right() - (n - idx) as f32 * bar_w;
        let h = (value / display_max).clamp(0.04, 1.0) * rect.height();
        let alpha = (80.0 + 150.0 * (idx as f32 + 1.0) / n as f32) as u8;
        let bar = egui::Rect::from_min_size(
            egui::pos2(x, rect.bottom() - h),
            Vec2::new((bar_w - gap).max(0.8), h),
        );
        painter.rect_filled(
            bar,
            Rounding::same(1.0),
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha),
        );
    }
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, Color32::from_rgb(30, 40, 52)),
    );
}

fn warning_banner(ui: &mut egui::Ui, text: &str) {
    egui::Frame::default()
        .fill(Color32::from_rgb(42, 24, 24))
        .stroke(Stroke::new(1.0, CORAL.gamma_multiply(0.6)))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).size(11.5).color(CORAL));
        });
}

fn paint_chip(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    painter.rect_stroke(rect, Rounding::same(6.0), stroke, StrokeKind::Inside);
    painter.rect_stroke(
        rect.shrink(4.5),
        Rounding::same(3.0),
        Stroke::new(1.0, color.gamma_multiply(0.55)),
        StrokeKind::Inside,
    );
    let pin_color = color.gamma_multiply(0.8);
    for i in 0..3 {
        let t = (i as f32 + 1.0) / 4.0;
        let x = rect.left() + rect.width() * t;
        let y = rect.top() + rect.height() * t;
        painter.line_segment(
            [egui::pos2(rect.left() - 3.0, x), egui::pos2(rect.left() + 1.5, x)],
            Stroke::new(1.0, pin_color),
        );
        painter.line_segment(
            [egui::pos2(rect.right() - 1.5, x), egui::pos2(rect.right() + 3.0, x)],
            Stroke::new(1.0, pin_color),
        );
        painter.line_segment(
            [egui::pos2(y, rect.top() - 3.0), egui::pos2(y, rect.top() + 1.5)],
            Stroke::new(1.0, pin_color),
        );
        painter.line_segment(
            [egui::pos2(y, rect.bottom() - 1.5), egui::pos2(y, rect.bottom() + 3.0)],
            Stroke::new(1.0, pin_color),
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "C:\\Windows\\Fonts\\deng.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                if let Some(list) = fonts.families.get_mut(&family) {
                    list.push("cjk".to_owned());
                }
            }
            break;
        }
    }
    ctx.set_fonts(fonts);
}

fn install_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = BG;
    style.visuals.extreme_bg_color = CODE_BG;
    style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.28);
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(12.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(10.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(22.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(11.5, FontFamily::Monospace),
    );
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn paint_background(ctx: &egui::Context) {
    let rect = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(rect, Rounding::ZERO, BG);
    let steps = 32;
    for i in 0..steps {
        let t = i as f32 / (steps - 1) as f32;
        let color = lerp_color(
            Color32::from_rgb(11, 16, 23),
            Color32::from_rgb(6, 9, 13),
            t,
        );
        let y0 = rect.top() + rect.height() * t;
        let y1 = rect.top() + rect.height() * (i + 1) as f32 / (steps - 1) as f32;
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(rect.left(), y0), egui::pos2(rect.right(), y1)),
            Rounding::ZERO,
            color,
        );
    }
    painter.circle_filled(
        egui::pos2(rect.right() - 40.0, rect.top() + 30.0),
        360.0,
        Color32::from_rgba_unmultiplied(62, 231, 195, 6),
    );
    painter.circle_filled(
        egui::pos2(rect.left() + 90.0, rect.bottom() - 80.0),
        300.0,
        Color32::from_rgba_unmultiplied(122, 162, 255, 6),
    );
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn freq_color(ratio: f32) -> Color32 {
    if ratio < 0.55 {
        ACCENT
    } else if ratio < 0.82 {
        GOLD
    } else {
        CORAL
    }
}

fn load_color(load: f32) -> Color32 {
    if load < 0.55 {
        ACCENT
    } else if load < 0.85 {
        GOLD
    } else {
        CORAL
    }
}

fn short_vendor(vendor: &str) -> &'static str {
    if vendor.contains("Intel") {
        "INTEL"
    } else if vendor.contains("AMD") {
        "AMD"
    } else if vendor.is_empty() {
        "CPU"
    } else {
        "OTHER"
    }
}

fn thousands(value: f32) -> String {
    let n = value.round() as i64;
    let negative = n < 0;
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if negative {
        out.insert(0, '-');
    }
    out
}

fn code_line_color(line: &str) -> Color32 {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("///") {
        CODE_COMMENT
    } else if trimmed.starts_with('#') {
        Color32::from_rgb(196, 168, 255)
    } else if trimmed.starts_with("pub ")
        || trimmed.starts_with("fn ")
        || trimmed.starts_with("impl ")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("use ")
        || trimmed.starts_with("const ")
        || trimmed.starts_with("unsafe ")
        || trimmed.starts_with("extern ")
        || trimmed.starts_with("mod ")
    {
        CODE_KEY
    } else {
        CODE_TEXT
    }
}

fn save_screenshot(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let [width, height] = image.size;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&pixel.to_array());
    }
    write_png(path, width as u32, height as u32, &rgba)
}

/// Minimal PNG writer (8-bit RGBA, uncompressed deflate blocks) so the app
/// does not need an image-decoding dependency just to save a screenshot.
fn write_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), String> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &ihdr);

    let row_bytes = width as usize * 4;
    let mut raw = Vec::with_capacity((row_bytes + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0); // filter: None
        let start = y * row_bytes;
        raw.extend_from_slice(&rgba[start..start + row_bytes]);
    }

    // zlib stream with stored (uncompressed) deflate blocks
    let mut z = Vec::new();
    z.extend_from_slice(&[0x78, 0x01]);
    let mut offset = 0;
    while offset < raw.len() {
        let remaining = raw.len() - offset;
        let len = remaining.min(65_535);
        let last = offset + len == raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(len as u16).to_le_bytes());
        z.extend_from_slice(&(!(len as u16)).to_le_bytes());
        z.extend_from_slice(&raw[offset..offset + len]);
        offset += len;
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    png_chunk(&mut out, b"IDAT", &z);
    png_chunk(&mut out, b"IEND", &[]);

    std::fs::write(path, out).map_err(|e| e.to_string())
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = 0xffff_ffffu32;
    crc = crc32(crc, kind);
    crc = crc32(crc, data);
    out.extend_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
}

fn crc32(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn parse_args() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let mut idx = 1;
    while idx < args.len() {
        if args[idx] == "--screenshot" {
            if let Some(value) = args.get(idx + 1) {
                return Some(PathBuf::from(value));
            }
        }
        if let Some(value) = args[idx].strip_prefix("--screenshot=") {
            return Some(PathBuf::from(value));
        }
        idx += 1;
    }
    None
}

fn main() -> eframe::Result<()> {
    let screenshot = parse_args();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 840.0])
            .with_min_inner_size([960.0, 640.0])
            .with_title("PulseCraft 主频监视器"),
        ..Default::default()
    };

    eframe::run_native(
        "pulsecraft",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, screenshot)))),
    )
}
