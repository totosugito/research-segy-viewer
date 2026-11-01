//! Thin `egui` / `eframe` user interface delegating all parsing, viewport geometry,
//! and progressive rendering to [`AppController`], `render`, and `segy_core`.

use eframe::egui::{
    self, Align2, Color32, ColorImage, Context, CornerRadius, FontId, Pos2, Rect, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui, Vec2,
};
use render::{
    AgcConfig, ClipStrategy, ColormapKind, DisplayMode, NormScope, Polarity, RenderLod,
    TraceBalance, WiggleFillMode,
};
use segy_core::AxisHeaderKey;

use crate::cli::AppCliArgs;
use crate::controller::AppController;

const TOP_AXIS_MARGIN_PX: f32 = 34.0;
const LEFT_AXIS_MARGIN_PX: f32 = 64.0;
const RIGHT_MARGIN_PX: f32 = 12.0;
const BOTTOM_MARGIN_PX: f32 = 10.0;

///Selectable horizontal axis header keys exposed in the UI combo box.
const AXIS_KEY_OPTIONS: [(AxisHeaderKey, &str); 7] = [
    (AxisHeaderKey::TraceNumber, "Trace #"),
    (AxisHeaderKey::SequenceLine, "Seq Line"),
    (AxisHeaderKey::FieldRecord, "Field Record"),
    (AxisHeaderKey::Cdp, "CDP / Ensemble"),
    (AxisHeaderKey::Offset, "Offset"),
    (AxisHeaderKey::Inline, "Inline"),
    (AxisHeaderKey::Crossline, "Crossline"),
];

/// Thin `eframe::App` wrapper around [`AppController`].
pub struct SegyViewerApp {
    controller: AppController,
    texture: Option<TextureHandle>,
    uploaded_generation: u64,
    show_sidebar: bool,
    show_headers_window: bool,
    show_grid: bool,
}

impl SegyViewerApp {
    /// Creates a new [`SegyViewerApp`] initialized with optional CLI arguments.
    pub fn new(args: AppCliArgs) -> Self {
        let mut controller = AppController::default();
        let _ = controller.apply_cli_args(&args);
        Self {
            controller,
            texture: None,
            uploaded_generation: 0,
            show_sidebar: true,
            show_headers_window: false,
            show_grid: true,
        }
    }

    /// Returns a shared reference to the underlying [`AppController`].
    #[inline]
    pub const fn controller(&self) -> &AppController {
        &self.controller
    }

    /// Returns a mutable reference to the underlying [`AppController`].
    #[inline]
    pub fn controller_mut(&mut self) -> &mut AppController {
        &mut self.controller
    }

    fn handle_dropped_files(&mut self, ctx: &Context) {
        let dropped_path =
            ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped_path {
            let _ = self.controller.open_file(path);
        }
    }

    fn open_file_dialog(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Open SEG-Y File")
            .add_filter("SEG-Y Seismic", &["sgy", "segy", "SGY", "SEGY"])
            .pick_file();
        if let Some(path) = picked {
            let _ = self.controller.open_file(path);
        }
    }

    fn save_png_dialog(&mut self) {
        if !self.controller.has_file() {
            return;
        }
        let picked = rfd::FileDialog::new()
            .set_title("Save Seismic Section PNG")
            .add_filter("PNG Image", &["png"])
            .set_file_name("seismic_section.png")
            .save_file();
        if let Some(path) = picked {
            let _ = self.controller.save_png(path);
        }
    }

    fn draw_top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open SEG-Y…").clicked() {
                self.open_file_dialog();
            }
            let save_enabled = self.controller.has_file();
            if ui
                .add_enabled(save_enabled, egui::Button::new("Save PNG…"))
                .clicked()
            {
                self.save_png_dialog();
            }
            ui.toggle_value(&mut self.show_sidebar, "Controls");
            if self.controller.has_file() {
                ui.toggle_value(&mut self.show_headers_window, "Headers");
            }

            ui.separator();
            ui.label("Mode:");
            let current_mode = self.controller.viewport().display_mode();
            for (mode, label) in [
                (DisplayMode::Wiggle, "Wiggle"),
                (DisplayMode::Image, "Image"),
                (DisplayMode::Overlay, "Overlay"),
            ] {
                if ui.selectable_label(current_mode == mode, label).clicked() {
                    self.controller.set_display_mode(mode);
                }
            }

            ui.separator();
            if ui.button("Fit All").clicked() {
                self.controller.fit_to_data();
            }
            if ui.button("Fit H").clicked() {
                self.controller.fit_horizontal();
            }
            if ui.button("Fit V").clicked() {
                self.controller.fit_vertical();
            }

            ui.separator();
            let pol = self.controller.render_params().amplitude.polarity;
            let pol_label = match pol {
                Polarity::Normal => "Polarity: Normal (+)",
                Polarity::Reversed => "Polarity: Reversed (−)",
            };
            if ui.button(pol_label).clicked() {
                self.controller.toggle_polarity();
            }

            ui.separator();
            let cmap_label = ui.label("Colormap:");
            let current_cmap = self.controller.render_params().colormap.kind;
            egui::ComboBox::from_id_salt("top_colormap_combo")
                .selected_text(current_cmap.label())
                .show_ui(ui, |ui| {
                    for kind in ColormapKind::ALL {
                        if ui
                            .selectable_label(current_cmap == kind, kind.label())
                            .clicked()
                        {
                            self.controller.set_colormap_kind(kind);
                        }
                    }
                })
                .response
                .labelled_by(cmap_label.id);
        });
    }

    fn draw_sidebar(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.draw_amplitude_section(ui);
            ui.separator();
            self.draw_color_section(ui);
            ui.separator();
            self.draw_wiggle_section(ui);
            ui.separator();
            self.draw_geometry_section(ui);
        });
    }

    fn draw_amplitude_section(&mut self, ui: &mut Ui) {
        ui.heading("Amplitude");
        let amp = self.controller.render_params().amplitude;

        let mut gain_db = amp.gain_db;
        ui.horizontal(|ui| {
            ui.label("Gain:");
            if ui.small_button("0 dB").clicked() {
                gain_db = 0.0;
                self.controller.set_gain_db(0.0);
            }
        });
        if ui
            .add(
                egui::Slider::new(&mut gain_db, -40.0..=40.0)
                    .step_by(0.5)
                    .text("Gain (dB)"),
            )
            .changed()
        {
            self.controller.set_gain_db(gain_db);
        }

        ui.horizontal(|ui| {
            ui.label("Scope:");
            if ui
                .selectable_label(amp.scope == NormScope::Global, "Global")
                .clicked()
            {
                self.controller.set_norm_scope(NormScope::Global);
            }
            if ui
                .selectable_label(amp.scope == NormScope::PerTrace, "Per-Trace")
                .clicked()
            {
                self.controller.set_norm_scope(NormScope::PerTrace);
            }
        });

        self.draw_clip_controls(ui, amp.clip);

        let mut dc_rem = amp.dc_removal;
        if ui.checkbox(&mut dc_rem, "DC Removal").changed() {
            self.controller.set_dc_removal(dc_rem);
        }

        let mut agc_on = amp.agc.enabled;
        let mut agc_win = amp.agc.window_samples;
        if ui.checkbox(&mut agc_on, "Sliding-Window AGC").changed() {
            let next_agc = if agc_on {
                AgcConfig::enabled(agc_win)
            } else {
                AgcConfig::off()
            };
            self.controller.set_agc(next_agc);
        }
        if agc_on
            && ui
                .add(egui::Slider::new(&mut agc_win, 10..=500).text("AGC Window (samples)"))
                .changed()
        {
            self.controller.set_agc(AgcConfig::enabled(agc_win));
        }

        ui.horizontal(|ui| {
            ui.label("Balance:");
            for (bal, name) in [
                (TraceBalance::Off, "Off"),
                (TraceBalance::Rms, "RMS Bal"),
                (TraceBalance::MeanAbs, "MeanAbs"),
                (TraceBalance::Peak, "Peak Bal"),
            ] {
                if ui
                    .selectable_label(amp.trace_balance == bal, name)
                    .clicked()
                {
                    self.controller.set_trace_balance(bal);
                }
            }
        });
    }

    fn draw_clip_controls(&mut self, ui: &mut Ui, clip: ClipStrategy) {
        ui.horizontal(|ui| {
            ui.label("Clip:");
            let is_pct = matches!(clip, ClipStrategy::Percentile { .. });
            let is_rms = matches!(clip, ClipStrategy::Rms { .. });
            let is_peak = matches!(clip, ClipStrategy::Peak);
            if ui.selectable_label(is_pct, "Percentile").clicked() {
                self.controller
                    .set_clip_strategy(ClipStrategy::Percentile { percentile: 99.0 });
            }
            if ui.selectable_label(is_rms, "RMS").clicked() {
                self.controller
                    .set_clip_strategy(ClipStrategy::Rms { multiple: 2.5 });
            }
            if ui.selectable_label(is_peak, "Peak").clicked() {
                self.controller.set_clip_strategy(ClipStrategy::Peak);
            }
        });

        match clip {
            ClipStrategy::Percentile { mut percentile } => {
                if ui
                    .add(egui::Slider::new(&mut percentile, 50.0..=100.0).text("Percentile (%)"))
                    .changed()
                {
                    self.controller
                        .set_clip_strategy(ClipStrategy::Percentile { percentile });
                }
            }
            ClipStrategy::Rms { mut multiple } => {
                if ui
                    .add(egui::Slider::new(&mut multiple, 0.2..=10.0).text("× RMS"))
                    .changed()
                {
                    self.controller
                        .set_clip_strategy(ClipStrategy::Rms { multiple });
                }
            }
            ClipStrategy::Absolute { mut limit } => {
                if ui
                    .add(egui::Slider::new(&mut limit, 0.01..=10_000.0).text("Abs Limit"))
                    .changed()
                {
                    self.controller
                        .set_clip_strategy(ClipStrategy::Absolute { limit });
                }
            }
            ClipStrategy::Peak => {}
        }
    }

    fn draw_color_section(&mut self, ui: &mut Ui) {
        ui.heading("Colormap");
        let cmap = self.controller.render_params().colormap;

        let mut rev = cmap.reversed;
        if ui.checkbox(&mut rev, "Reverse Colormap").changed() {
            self.controller.set_colormap_reversed(rev);
        }

        let mut contrast = cmap.contrast;
        if ui
            .add(egui::Slider::new(&mut contrast, 0.2..=5.0).text("Contrast"))
            .changed()
        {
            self.controller.set_colormap_contrast(contrast);
        }

        let mut bias = cmap.bias;
        if ui
            .add(egui::Slider::new(&mut bias, -0.8..=0.8).text("Bias"))
            .changed()
        {
            self.controller.set_colormap_bias(bias);
        }
    }

    fn draw_wiggle_section(&mut self, ui: &mut Ui) {
        ui.heading("Wiggle / VA");
        let params = *self.controller.render_params();

        ui.horizontal(|ui| {
            ui.label("Fill:");
            for (fill, label) in [
                (WiggleFillMode::Positive, "Pos"),
                (WiggleFillMode::Negative, "Neg"),
                (WiggleFillMode::Both, "Both"),
                (WiggleFillMode::LineOnly, "None"),
            ] {
                if ui
                    .selectable_label(params.wiggle_fill == fill, label)
                    .clicked()
                {
                    self.controller.set_wiggle_fill(fill);
                }
            }
        });

        let mut draw_line = params.wiggle_draw_line;
        if ui.checkbox(&mut draw_line, "Draw Wiggle Line").changed() {
            self.controller.set_wiggle_draw_line(draw_line);
        }

        let mut exc = params.wiggle_excursion_spacings;
        if ui
            .add(egui::Slider::new(&mut exc, 0.2..=4.0).text("Excursion"))
            .changed()
        {
            self.controller.set_wiggle_excursion_spacings(exc);
        }

        let mut clip_sp = params.wiggle_clip_spacings;
        if ui
            .add(egui::Slider::new(&mut clip_sp, 0.5..=5.0).text("Max Excursion"))
            .changed()
        {
            self.controller.set_wiggle_clip_spacings(clip_sp);
        }

        let mut auto_sw = self.controller.viewport().auto_switch_wiggle_to_image();
        if ui
            .checkbox(&mut auto_sw, "Auto-switch Wiggle -> Image (<2 px)")
            .changed()
        {
            self.controller.set_auto_switch_wiggle_to_image(auto_sw);
        }
    }

    fn draw_geometry_section(&mut self, ui: &mut Ui) {
        ui.heading("Axes & Geometry");
        let current_key = self.controller.viewport().horizontal_axis_key();
        let current_label = AXIS_KEY_OPTIONS
            .iter()
            .find(|(k, _)| *k == current_key)
            .map_or("Custom", |(_, l)| *l);

        ui.horizontal(|ui| {
            let xaxis_label = ui.label("X-Axis:");
            egui::ComboBox::from_id_salt("xaxis_key_combo")
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    for (key, label) in AXIS_KEY_OPTIONS {
                        if ui.selectable_label(current_key == key, label).clicked() {
                            self.controller.set_horizontal_axis_key(key);
                        }
                    }
                })
                .response
                .labelled_by(xaxis_label.id);
        });

        let mut decimate = self.controller.viewport().trace_decimate_step();
        if ui
            .add(egui::Slider::new(&mut decimate, 1..=32).text("Trace Step"))
            .changed()
        {
            self.controller.set_trace_decimate_step(decimate);
        }

        let mut rev_traces = self.controller.viewport().reverse_traces();
        if ui
            .checkbox(&mut rev_traces, "Reverse Trace Order")
            .changed()
        {
            self.controller.set_reverse_traces(rev_traces);
        }

        ui.checkbox(&mut self.show_grid, "Show Axis Gridlines");
    }

    fn draw_status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(err) = self.controller.last_error() {
                ui.colored_label(Color32::from_rgb(220, 50, 50), err);
                if ui.small_button("Dismiss").clicked() {
                    self.controller.clear_error();
                }
                ui.separator();
            } else if let Some(status) = self.controller.status_message() {
                ui.label(status);
                ui.separator();
            }

            if self.controller.has_file() {
                let vp = self.controller.viewport();
                let lod_tag = match self.controller.rendered_lod() {
                    Some(RenderLod::Coarse) => "LOD: Coarse…",
                    Some(RenderLod::Full) => "LOD: Full",
                    None => "LOD: Pending",
                };
                ui.label(format!(
                    "{lod_tag} | Zoom H:{:.1}× V:{:.1}× | Mode: {:?}",
                    vp.zoom_ratio_horizontal(),
                    vp.zoom_ratio_vertical(),
                    vp.effective_display_mode(),
                ));
                ui.separator();
            }

            if let Some(cursor) = self.controller.last_cursor_readout() {
                ui.monospace(format!(
                    "Trace #{} (Hdr: {}) | Sample #{} | Time: {:.1} ms | Amp: {:.5e}",
                    cursor.trace_number,
                    cursor.header_value,
                    cursor.sample_index,
                    cursor.time_ms,
                    cursor.amplitude,
                ));
            } else {
                ui.label("Hover over section for cursor readout (Drag: Pan, Wheel: Zoom, Shift+Wheel: Zoom H, Alt+Wheel: Zoom V)");
            }
        });
    }

    fn draw_central_canvas(&mut self, ctx: &Context, ui: &mut Ui) {
        let avail = ui.available_rect_before_wrap();
        if !self.controller.has_file() {
            ui.centered_and_justified(|ui| {
                ui.label("Open a SEG-Y file via 'Open SEG-Y…', drag & drop, or launch with `app <file.sgy>`.");
            });
            return;
        }

        let plot_left = (avail.min.x + LEFT_AXIS_MARGIN_PX).min(avail.max.x - 16.0);
        let plot_top = (avail.min.y + TOP_AXIS_MARGIN_PX).min(avail.max.y - 16.0);
        let plot_right = (avail.max.x - RIGHT_MARGIN_PX).max(plot_left + 16.0);
        let plot_bottom = (avail.max.y - BOTTOM_MARGIN_PX).max(plot_top + 16.0);
        let plot_rect = Rect::from_min_max(
            Pos2::new(plot_left, plot_top),
            Pos2::new(plot_right, plot_bottom),
        );

        let plot_w = (plot_rect.width().round().max(1.0)) as u32;
        let plot_h = (plot_rect.height().round().max(1.0)) as u32;
        self.controller.set_viewport_dimensions(plot_w, plot_h);

        if self.controller.poll_background_render() {
            ctx.request_repaint_after(std::time::Duration::from_millis(8));
        }
        self.sync_texture_if_needed(ctx);

        let response = ui.allocate_rect(plot_rect, Sense::click_and_drag());
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "Seismic Canvas"));
        self.handle_plot_interactions(ctx, &response, plot_rect);

        let painter = ui.painter();
        painter.rect_filled(avail, CornerRadius::ZERO, Color32::from_rgb(28, 30, 34));

        if let Some(tex) = &self.texture {
            let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            painter.image(tex.id(), plot_rect, uv, Color32::WHITE);
        }

        self.paint_axes_and_grid(painter, plot_rect);
        if let Some(hover_pos) = response.hover_pos()
            && plot_rect.contains(hover_pos)
        {
            self.paint_cursor_crosshair(painter, plot_rect, hover_pos);
        }
    }

    fn sync_texture_if_needed(&mut self, ctx: &Context) {
        let current_gen = self.controller.image_generation();
        if current_gen == self.uploaded_generation {
            return;
        }
        let Some(rendered) = self.controller.cached_image() else {
            return;
        };
        let color_img = ColorImage::from_rgba_unmultiplied(
            [rendered.width as usize, rendered.height as usize],
            &rendered.rgba,
        );
        if let Some(tex) = &mut self.texture {
            tex.set(color_img, TextureOptions::NEAREST);
        } else {
            self.texture = Some(ctx.load_texture(
                "seismic_section_viewport",
                color_img,
                TextureOptions::NEAREST,
            ));
        }
        self.uploaded_generation = current_gen;
    }

    fn handle_plot_interactions(
        &mut self,
        ctx: &Context,
        response: &egui::Response,
        plot_rect: Rect,
    ) {
        if response.double_clicked() {
            self.controller.fit_to_data();
        }

        if response.dragged_by(egui::PointerButton::Primary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            let delta = response.drag_delta();
            self.controller.pan_by_screen_drag_px(delta.x, delta.y);
        }

        if let Some(hover_pos) = response.hover_pos()
            && plot_rect.contains(hover_pos)
        {
            let local_x = (hover_pos.x - plot_rect.min.x).clamp(0.0, plot_rect.width() - 0.01);
            let local_y = (hover_pos.y - plot_rect.min.y).clamp(0.0, plot_rect.height() - 0.01);
            self.controller.update_cursor_at_pixel(local_x, local_y);

            let (scroll_y, zoom_delta, modifiers) =
                ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.modifiers));

            let factor = if (zoom_delta - 1.0).abs() > 1e-3 {
                f64::from(zoom_delta)
            } else if scroll_y.abs() > 0.1 {
                1.002_f64.powf(f64::from(scroll_y))
            } else {
                1.0
            };

            if (factor - 1.0).abs() > 1e-3 {
                let zoom_h = if modifiers.alt && !modifiers.shift {
                    1.0
                } else {
                    factor
                };
                let zoom_v = if modifiers.shift && !modifiers.alt {
                    1.0
                } else {
                    factor
                };
                self.controller
                    .zoom_at_pixel(zoom_h, zoom_v, local_x, local_y);
            }
        } else {
            self.controller.clear_cursor();
        }
    }

    fn paint_axes_and_grid(&self, painter: &egui::Painter, plot_rect: Rect) {
        let axis_stroke = Stroke::new(1.0, Color32::from_rgb(170, 175, 185));
        let grid_stroke = Stroke::new(0.5, Color32::from_rgba_unmultiplied(120, 130, 140, 70));
        let label_color = Color32::from_rgb(225, 230, 238);
        let font = FontId::monospace(11.0);

        painter.rect_stroke(
            plot_rect,
            CornerRadius::ZERO,
            axis_stroke,
            StrokeKind::Outside,
        );

        // Horizontal trace/header axis along the top of plot_rect
        let trace_ticks = self.controller.trace_axis_ticks(90.0);
        for tick in &trace_ticks {
            let x = (plot_rect.min.x + tick.position_px).clamp(plot_rect.min.x, plot_rect.max.x);
            painter.line_segment(
                [
                    Pos2::new(x, plot_rect.min.y - 5.0),
                    Pos2::new(x, plot_rect.min.y),
                ],
                axis_stroke,
            );
            if self.show_grid {
                painter.line_segment(
                    [Pos2::new(x, plot_rect.min.y), Pos2::new(x, plot_rect.max.y)],
                    grid_stroke,
                );
            }
            painter.text(
                Pos2::new(x, plot_rect.min.y - 8.0),
                Align2::CENTER_BOTTOM,
                &tick.label,
                font.clone(),
                label_color,
            );
        }

        // Vertical time (ms) axis along the left of plot_rect
        let time_ticks = self.controller.time_axis_ticks(54.0);
        for tick in &time_ticks {
            let y = (plot_rect.min.y + tick.position_px).clamp(plot_rect.min.y, plot_rect.max.y);
            painter.line_segment(
                [
                    Pos2::new(plot_rect.min.x - 5.0, y),
                    Pos2::new(plot_rect.min.x, y),
                ],
                axis_stroke,
            );
            if self.show_grid {
                painter.line_segment(
                    [Pos2::new(plot_rect.min.x, y), Pos2::new(plot_rect.max.x, y)],
                    grid_stroke,
                );
            }
            painter.text(
                Pos2::new(plot_rect.min.x - 8.0, y),
                Align2::RIGHT_CENTER,
                &tick.label,
                font.clone(),
                label_color,
            );
        }

        // Corner axis unit label
        painter.text(
            Pos2::new(plot_rect.min.x - 8.0, plot_rect.min.y - 8.0),
            Align2::RIGHT_BOTTOM,
            "Time (ms)",
            font,
            Color32::from_rgb(160, 170, 185),
        );
    }

    fn paint_cursor_crosshair(&self, painter: &egui::Painter, plot_rect: Rect, hover_pos: Pos2) {
        let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 210, 60, 160));
        painter.line_segment(
            [
                Pos2::new(hover_pos.x, plot_rect.min.y),
                Pos2::new(hover_pos.x, plot_rect.max.y),
            ],
            stroke,
        );
        painter.line_segment(
            [
                Pos2::new(plot_rect.min.x, hover_pos.y),
                Pos2::new(plot_rect.max.x, hover_pos.y),
            ],
            stroke,
        );
    }

    fn draw_headers_window(&mut self, ctx: &Context) {
        if !self.show_headers_window {
            return;
        }
        let Some(file) = self.controller.file().cloned() else {
            self.show_headers_window = false;
            return;
        };

        let mut open = self.show_headers_window;
        egui::Window::new("SEG-Y File Headers (Read-Only)")
            .open(&mut open)
            .default_size(Vec2::new(660.0, 460.0))
            .show(ctx, |ui| {
                let bh = file.binary_header();
                let (rev_maj, rev_min) = bh.revision_major_minor();
                ui.label(format!(
                    "Traces: {} | Samples/Trace: {} | Interval: {} µs | Format: {:?} | Rev: {}.{} | Encoding: {:?}",
                    file.trace_count(),
                    bh.samples_per_trace,
                    bh.sample_interval_us,
                    bh.sample_format,
                    rev_maj,
                    rev_min,
                    file.textual_header().encoding(),
                ));
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for line in file.textual_header().lines() {
                        ui.monospace(line);
                    }
                });
            });
        self.show_headers_window = open;
    }

    /// Renders the complete application UI for a single frame into `ui`.
    pub fn draw_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);

        egui::Panel::top("top_toolbar").show(ui, |ui| {
            self.draw_top_bar(ui);
        });

        egui::Panel::bottom("bottom_status_bar").show(ui, |ui| {
            self.draw_status_bar(ui);
        });

        if self.show_sidebar {
            egui::Panel::left("left_controls_panel")
                .default_size(250.0)
                .resizable(true)
                .show(ui, |ui| {
                    self.draw_sidebar(ui);
                });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            self.draw_central_canvas(&ctx, ui);
        });

        self.draw_headers_window(&ctx);
    }
}

impl eframe::App for SegyViewerApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw_ui(ui);
    }
}
