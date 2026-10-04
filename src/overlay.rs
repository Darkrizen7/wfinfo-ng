use std::{
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, Frame, Margin, Pos2, RichText, Stroke, Vec2,
    ViewportBuilder, ViewportCommand, X11WindowType,
};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::FontId;

use crate::{database::RelicAdvice, ocr::PartRect, wfinfo_data::item_data::Refinement};

const GOLD: Color32 = Color32::from_rgb(255, 200, 60);
const GREEN: Color32 = Color32::from_rgb(110, 220, 120);
const ORANGE: Color32 = Color32::from_rgb(255, 150, 70);

/// Below this many trades a day, an item may take a while to sell
const LOW_VOLUME: f32 = 5.0;

/// Something to draw above a piece of text on screen
#[derive(Clone, Debug)]
pub enum Label {
    Reward(RewardLabel),
    Relic(RelicLabel),
}

impl Label {
    fn rect(&self) -> PartRect {
        match self {
            Label::Reward(label) => label.rect,
            Label::Relic(label) => label.rect,
        }
    }
}

/// Value estimate and refinement advice to display above one relic
#[derive(Clone, Debug)]
pub struct RelicLabel {
    /// Where the relic's name is on screen, in pixels relative to the game window
    pub rect: PartRect,
    /// `None` when the refinement isn't written on screen
    pub refinement: Option<Refinement>,
    /// `None` when the relic's drops are missing from the downloaded data
    pub advice: Option<RelicAdvice>,
    /// Show every refinement level instead of a summary
    pub detailed: bool,
}

/// Price information to display above one reward
#[derive(Clone, Debug)]
pub struct RewardLabel {
    /// Where the reward's name is on screen, in pixels relative to the game window
    pub rect: PartRect,
    /// `None` when the item wasn't recognized
    pub name: Option<String>,
    pub platinum: f32,
    /// Ducat value converted to platinum
    pub ducats_platinum: f32,
    /// Number of trades on the previous day
    pub volume: f32,
    pub vaulted: bool,
    pub best: bool,
}

/// Game window geometry in physical pixels
#[derive(Clone, Copy, Debug)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub struct OverlayOptions {
    pub geometry: WindowGeometry,
    /// How long labels stay visible after a detection
    pub display_duration: Duration,
    /// Distance in pixels between the label and the top of the reward's name
    pub vertical_offset: f32,
    /// Drawn behind the labels, used to check label placement on a screenshot
    pub background: Option<ColorImage>,
}

/// Runs the overlay on the current thread (must be the main thread) until it is closed.
///
/// `on_ready` is called with a sender for new labels once the window exists.
pub fn run_overlay(
    options: OverlayOptions,
    on_ready: impl FnOnce(OverlayHandle) + 'static,
) -> eframe::Result {
    let geometry = options.geometry;
    let viewport = ViewportBuilder::default()
        .with_title("WFInfo overlay")
        .with_position(Pos2::new(geometry.x as f32, geometry.y as f32))
        .with_inner_size(Vec2::new(geometry.width as f32, geometry.height as f32))
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_mouse_passthrough(true)
        .with_active(false)
        .with_taskbar(false)
        .with_resizable(false)
        // Ignored by the window manager: stays above the (fullscreen) game and never takes focus
        .with_override_redirect(true)
        .with_window_type(X11WindowType::Notification);
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "wfinfo-overlay",
        native_options,
        Box::new(move |cc| {
            let (sender, receiver) = mpsc::channel();
            on_ready(OverlayHandle {
                sender,
                ctx: cc.egui_ctx.clone(),
            });
            Ok(Box::new(OverlayApp {
                options,
                receiver,
                labels: Vec::new(),
                expires_at: None,
                applied_scale: None,
                background: None,
            }))
        }),
    )
}

/// Used from other threads to update the overlay
#[derive(Clone)]
pub struct OverlayHandle {
    sender: Sender<(Vec<Label>, bool)>,
    ctx: egui::Context,
}

impl OverlayHandle {
    /// Replaces the labels on screen, they disappear after the configured display duration
    pub fn show(&self, labels: Vec<Label>) {
        self.send(labels, true);
    }

    /// Replaces the labels on screen, they stay until the next call to `show*`
    pub fn show_until_replaced(&self, labels: Vec<Label>) {
        self.send(labels, false);
    }

    fn send(&self, labels: Vec<Label>, expires: bool) {
        if self.sender.send((labels, expires)).is_ok() {
            self.ctx.request_repaint();
        }
    }
}

struct OverlayApp {
    options: OverlayOptions,
    receiver: Receiver<(Vec<Label>, bool)>,
    labels: Vec<Label>,
    /// `None` when the labels stay until replaced
    expires_at: Option<Instant>,
    /// Pixels per point the window geometry was last computed for
    applied_scale: Option<f32>,
    background: Option<egui::TextureHandle>,
}

impl OverlayApp {
    /// Window geometry is known in physical pixels, egui wants points
    fn fit_to_game_window(&mut self, ctx: &egui::Context) {
        let scale = ctx.pixels_per_point();
        if self.applied_scale == Some(scale) {
            return;
        }
        self.applied_scale = Some(scale);
        let geometry = self.options.geometry;
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::new(
            geometry.x as f32 / scale,
            geometry.y as f32 / scale,
        )));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(
            geometry.width as f32 / scale,
            geometry.height as f32 / scale,
        )));
    }
}

impl eframe::App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.fit_to_game_window(&ctx);

        if let Some(image) = self.options.background.take() {
            self.background =
                Some(ctx.load_texture("background", image, egui::TextureOptions::LINEAR));
        }
        if let Some(background) = &self.background {
            ui.painter().image(
                background.id(),
                ctx.content_rect(),
                egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        while let Ok((labels, expires)) = self.receiver.try_recv() {
            self.labels = labels;
            self.expires_at = expires.then(|| Instant::now() + self.options.display_duration);
        }

        if let Some(expires_at) = self.expires_at {
            let now = Instant::now();
            if now >= expires_at {
                self.labels.clear();
                self.expires_at = None;
                return;
            }
            ctx.request_repaint_after(expires_at - now);
        }

        let scale = ctx.pixels_per_point();
        for (index, label) in self.labels.iter().enumerate() {
            let rect = label.rect();
            let anchor = Pos2::new(
                (rect.x + rect.width / 2.0) / scale,
                (rect.y - self.options.vertical_offset) / scale,
            );
            egui::Area::new(egui::Id::new(("label", index)))
                .fixed_pos(anchor)
                .pivot(Align2::CENTER_BOTTOM)
                .interactable(false)
                .show(&ctx, |ui| match label {
                    Label::Reward(label) => draw_reward_label(ui, label),
                    Label::Relic(label) => draw_relic_label(ui, label),
                });
        }
    }
}

fn draw_reward_label(ui: &mut egui::Ui, label: &RewardLabel) {
    let stroke = if label.best {
        Stroke::new(2.0, GOLD)
    } else {
        Stroke::new(1.0, Color32::from_gray(90))
    };
    Frame::new()
        .fill(Color32::from_black_alpha(210))
        .stroke(stroke)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                if label.name.is_none() {
                    ui.label(RichText::new("?").size(22.0).color(Color32::GRAY));
                    return;
                }
                let color = if label.best { GOLD } else { Color32::WHITE };
                let star = if label.best { "★ " } else { "" };
                ui.label(
                    RichText::new(format!("{star}{} p", format_platinum(label.platinum)))
                        .size(22.0)
                        .strong()
                        .color(color),
                );
                ui.label(
                    RichText::new(format!(
                        "ducats: {} p",
                        format_platinum(label.ducats_platinum)
                    ))
                    .size(14.0)
                    .color(Color32::LIGHT_GRAY),
                );
                let volume_color = if label.volume < LOW_VOLUME {
                    ORANGE
                } else {
                    Color32::LIGHT_GRAY
                };
                let mut details = LayoutJob::default();
                let format = |color| TextFormat::simple(FontId::proportional(12.0), color);
                details.append(
                    &format!("{} sold/day", label.volume),
                    0.0,
                    format(volume_color),
                );
                if label.vaulted {
                    details.append(" · vaulted", 0.0, format(GOLD));
                }
                ui.label(details);
            });
        });
}

fn refinement_name(refinement: Refinement) -> &'static str {
    match refinement {
        Refinement::Intact => "Intact",
        Refinement::Exceptional => "Exceptional",
        Refinement::Flawless => "Flawless",
        Refinement::Radiant => "Radiant",
    }
}

fn draw_relic_label(ui: &mut egui::Ui, label: &RelicLabel) {
    let Some(advice) = &label.advice else {
        Frame::new()
            .fill(Color32::from_black_alpha(210))
            .stroke(Stroke::new(1.0, Color32::from_gray(90)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("? p")
                            .size(18.0)
                            .strong()
                            .color(Color32::GRAY),
                    );
                    ui.label(
                        RichText::new("Unknown rewards")
                            .size(12.0)
                            .color(Color32::GRAY),
                    );
                });
            });
        return;
    };
    let current = label.refinement.unwrap_or(Refinement::Intact);
    let recommendation = advice.recommendation(current);
    let stroke = if recommendation.is_some() {
        Stroke::new(2.0, GREEN)
    } else {
        Stroke::new(1.0, Color32::from_gray(90))
    };
    Frame::new()
        .fill(Color32::from_black_alpha(210))
        .stroke(stroke)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(format!(
                        "≈ {} p",
                        format_platinum(advice.value(current).platinum)
                    ))
                    .size(18.0)
                    .strong()
                    .color(Color32::WHITE),
                );
                if label.detailed {
                    for value in advice.values {
                        let recommended = recommendation.map(|(refinement, _)| refinement);
                        let color = if Some(value.refinement) == recommended {
                            GREEN
                        } else {
                            Color32::LIGHT_GRAY
                        };
                        let per_trace = if value.refinement == Refinement::Intact {
                            String::new()
                        } else {
                            format!("  ({:+.3} p/trace)", value.platinum_per_trace)
                        };
                        ui.label(
                            RichText::new(format!(
                                "{} : {} p{per_trace}",
                                refinement_name(value.refinement),
                                format_platinum(value.platinum)
                            ))
                            .size(13.0)
                            .color(color),
                        );
                    }
                }
                let advice_text = if let Some((refinement, per_trace)) = recommendation {
                    RichText::new(format!(
                        "Refine → {} ({per_trace:+.3} p/trace)",
                        refinement_name(refinement),
                    ))
                    .color(GREEN)
                } else {
                    RichText::new("Not worth refining").color(Color32::GRAY)
                };
                ui.label(advice_text.size(12.0));
            });
        });
}

/// One decimal at most: 11.666667 -> "11.7", 3.0 -> "3"
fn format_platinum(value: f32) -> String {
    let rounded = format!("{value:.1}");
    rounded
        .strip_suffix(".0")
        .map(str::to_owned)
        .unwrap_or(rounded)
}
