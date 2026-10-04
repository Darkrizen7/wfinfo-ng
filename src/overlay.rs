use std::{
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, Frame, Margin, Pos2, RichText, Stroke, Vec2,
    ViewportBuilder, ViewportCommand, X11WindowType,
};

use crate::ocr::PartRect;

const GOLD: Color32 = Color32::from_rgb(255, 200, 60);

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
                shown_at: None,
                applied_scale: None,
                background: None,
            }))
        }),
    )
}

/// Used from other threads to update the overlay
#[derive(Clone)]
pub struct OverlayHandle {
    sender: Sender<Vec<RewardLabel>>,
    ctx: egui::Context,
}

impl OverlayHandle {
    pub fn show(&self, labels: Vec<RewardLabel>) {
        if self.sender.send(labels).is_ok() {
            self.ctx.request_repaint();
        }
    }
}

struct OverlayApp {
    options: OverlayOptions,
    receiver: Receiver<Vec<RewardLabel>>,
    labels: Vec<RewardLabel>,
    shown_at: Option<Instant>,
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

        while let Ok(labels) = self.receiver.try_recv() {
            self.labels = labels;
            self.shown_at = Some(Instant::now());
        }

        let Some(shown_at) = self.shown_at else {
            return;
        };
        let elapsed = shown_at.elapsed();
        if elapsed >= self.options.display_duration {
            self.labels.clear();
            self.shown_at = None;
            return;
        }
        ctx.request_repaint_after(self.options.display_duration - elapsed);

        let scale = ctx.pixels_per_point();
        for (index, label) in self.labels.iter().enumerate() {
            let anchor = Pos2::new(
                (label.rect.x + label.rect.width / 2.0) / scale,
                (label.rect.y - self.options.vertical_offset) / scale,
            );
            egui::Area::new(egui::Id::new(("reward", index)))
                .fixed_pos(anchor)
                .pivot(Align2::CENTER_BOTTOM)
                .interactable(false)
                .show(&ctx, |ui| draw_label(ui, label));
        }
    }
}

fn draw_label(ui: &mut egui::Ui, label: &RewardLabel) {
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
                    RichText::new(format!("ducats: {} p", format_platinum(label.ducats_platinum)))
                        .size(14.0)
                        .color(Color32::LIGHT_GRAY),
                );
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
