//! Primary-feature script driven by *real* input events (pointer moves, clicks, drags, wheel and
//! keys) injected into the egui UI through `egui_kittest`. It is shared by the headless test
//! (`tests/primary_features.rs`) and the video example (`primary_demo`): both run exactly the same
//! gestures and the same checks; only the frame sink differs.

#![allow(dead_code)]

use app::{AppCliArgs, DemoOverlay, SegyViewerApp};
use eframe::egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2, Rect, TouchPhase,
    Vec2,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use render::DisplayMode;
use segy_core::AxisHeaderKey;

pub const WINDOW: Vec2 = Vec2::new(1280.0, 800.0);
/// Duration of one pointer-animation frame in the video.
pub const MOVE_FRAME_MS: u64 = 80;
const MOVE_FRAMES: usize = 12;

pub type R = Result<(), String>;

/// Receives a frame after every injected input step (video: render and save; test: ignore).
pub trait Sink {
    fn frame(&mut self, h: &mut Harness<'static, SegyViewerApp>, ms: u64) -> R;
}

pub struct Player<S: Sink> {
    pub h: Harness<'static, SegyViewerApp>,
    pub sink: S,
    overlay: DemoOverlay,
    pointer: Pos2,
    started: bool,
}

pub fn build_app(sgy: &str) -> Result<SegyViewerApp, String> {
    let args = AppCliArgs::parse_from([sgy]).map_err(|e| e.to_string())?;
    Ok(SegyViewerApp::new(args))
}

impl<S: Sink> Player<S> {
    pub fn new(h: Harness<'static, SegyViewerApp>, sink: S) -> Self {
        Self {
            h,
            sink,
            overlay: DemoOverlay::default(),
            pointer: Pos2::new(WINDOW.x * 0.6, WINDOW.y * 0.5),
            started: false,
        }
    }

    fn app(&self) -> &SegyViewerApp {
        self.h.state()
    }

    fn ensure(cond: bool, msg: impl Into<String>) -> R {
        if cond { Ok(()) } else { Err(msg.into()) }
    }

    fn emit(&mut self, ms: u64) -> R {
        self.h
            .state_mut()
            .set_demo_overlay(Some(self.overlay.clone()));
        self.h.step();
        self.sink.frame(&mut self.h, ms)
    }

    pub fn caption(&mut self, index: usize, total: usize, text: &str) {
        self.overlay.caption = text.to_string();
        self.overlay.index = index;
        self.overlay.total = total;
    }

    pub fn hold(&mut self, ms: u64) -> R {
        self.emit(ms)
    }

    fn set_pointer(&mut self, p: Pos2) {
        self.pointer = p;
        self.started = true;
        self.overlay.pointer = Some((p.x, p.y));
        self.h.event(Event::PointerMoved(p));
    }

    pub fn move_to(&mut self, to: Pos2) -> R {
        let from = self.pointer;
        if !self.started {
            self.set_pointer(from);
            self.emit(MOVE_FRAME_MS)?;
        }
        for i in 1..=MOVE_FRAMES {
            let t = i as f32 / MOVE_FRAMES as f32;
            let eased = t * t * (3.0 - 2.0 * t);
            self.set_pointer(from + (to - from) * eased);
            self.emit(MOVE_FRAME_MS)?;
        }
        Ok(())
    }

    fn button(&mut self, button: PointerButton, pressed: bool) -> R {
        self.overlay.pressed = pressed;
        self.h.event(Event::PointerButton {
            pos: self.pointer,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        });
        self.emit(if pressed { 160 } else { 120 })
    }

    pub fn click_at(&mut self, pos: Pos2) -> R {
        self.move_to(pos)?;
        self.button(PointerButton::Primary, true)?;
        self.button(PointerButton::Primary, false)
    }

    pub fn drag(&mut self, button: PointerButton, from: Pos2, to: Pos2) -> R {
        self.move_to(from)?;
        self.button(button, true)?;
        let start = self.pointer;
        for i in 1..=MOVE_FRAMES {
            let t = i as f32 / MOVE_FRAMES as f32;
            self.set_pointer(start + (to - start) * t);
            self.emit(MOVE_FRAME_MS)?;
        }
        self.button(button, false)
    }

    pub fn wheel(&mut self, at: Pos2, dy_per_frame: f32, frames: usize) -> R {
        self.move_to(at)?;
        for _ in 0..frames {
            self.h.event(Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, dy_per_frame),
                phase: TouchPhase::Move,
                source: MouseWheelSource::Wheel,
                modifiers: Modifiers::NONE,
            });
            self.emit(MOVE_FRAME_MS)?;
        }
        // egui spreads a wheel delta over several frames; let it finish.
        for _ in 0..6 {
            self.emit(MOVE_FRAME_MS)?;
        }
        Ok(())
    }

    pub fn key(&mut self, key: Key) -> R {
        self.h.event(Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        self.emit(500)
    }

    fn label_rect(&mut self, label: &str) -> Result<Rect, String> {
        self.h
            .query_all_by_label(label)
            .next()
            .map(|n| n.rect())
            .ok_or_else(|| format!("widget '{label}' not found"))
    }

    pub fn click_label(&mut self, label: &str) -> R {
        let rect = self.label_rect(label)?;
        self.click_at(rect.center())
    }

    fn canvas(&mut self) -> Result<Rect, String> {
        self.label_rect("Seismic Canvas")
    }

    fn at(&mut self, fx: f32, fy: f32) -> Result<Pos2, String> {
        let c = self.canvas()?;
        Ok(Pos2::new(
            c.min.x + c.width() * fx,
            c.min.y + c.height() * fy,
        ))
    }

    fn traces(&self) -> usize {
        self.app()
            .controller()
            .viewport()
            .visible_trace_range()
            .len()
    }

    fn mode(&self) -> DisplayMode {
        self.app().controller().viewport().display_mode()
    }

    /// Runs the whole primary-feature script, stopping at the first failed check.
    pub fn run_primary(&mut self) -> R {
        const TOTAL: usize = 12;
        let all_traces = self.traces();
        Self::ensure(all_traces > 100, "sample file not loaded")?;

        self.caption(
            1,
            TOTAL,
            "Primary features, driven by real mouse and keyboard input",
        );
        self.hold(2800)?;

        self.caption(
            2,
            TOTAL,
            "Hover: live readout of trace, header value, time and amplitude",
        );
        let p = self.at(0.30, 0.35)?;
        self.move_to(p)?;
        Self::ensure(
            self.app().controller().last_cursor_readout().is_some(),
            "hover produced no readout",
        )?;
        self.hold(1800)?;

        self.caption(3, TOTAL, "Mouse wheel zooms around the pointer");
        let p = self.at(0.50, 0.40)?;
        self.wheel(p, 55.0, 8)?;
        let zoomed = self.traces();
        Self::ensure(
            zoomed < all_traces,
            format!("wheel did not zoom ({zoomed})"),
        )?;
        self.hold(1800)?;

        self.caption(4, TOTAL, "Right-button drag pans the section");
        let before = self.app().controller().viewport().visible_trace_range();
        let (a, b) = (self.at(0.65, 0.5)?, self.at(0.30, 0.4)?);
        self.drag(PointerButton::Secondary, a, b)?;
        Self::ensure(
            self.app().controller().viewport().visible_trace_range() != before,
            "right-drag did not pan",
        )?;
        self.hold(1800)?;

        self.caption(5, TOTAL, "Fit All returns to the full section");
        self.click_label("Fit All")?;
        Self::ensure(
            self.traces() == all_traces,
            "Fit All did not restore the view",
        )?;
        self.hold(1800)?;

        self.caption(6, TOTAL, "Zoom box: drag a rectangle with the left button");
        let (a, b) = (self.at(0.50, 0.30)?, self.at(0.54, 0.62)?);
        self.drag(PointerButton::Primary, a, b)?;
        let boxed = self.traces();
        Self::ensure(
            boxed < all_traces / 10,
            format!("zoom box left {boxed} traces"),
        )?;
        self.hold(2500)?;

        self.caption(7, TOTAL, "Plot type: Wiggle");
        self.click_label("Wiggle")?;
        Self::ensure(
            self.mode() == DisplayMode::Wiggle,
            "Wiggle button had no effect",
        )?;
        self.hold(2500)?;
        self.caption(7, TOTAL, "Plot type: Overlay (wiggle over image)");
        self.click_label("Overlay")?;
        Self::ensure(
            self.mode() == DisplayMode::Overlay,
            "Overlay button had no effect",
        )?;
        self.hold(2500)?;
        self.caption(7, TOTAL, "Plot type: Image (variable density)");
        self.click_label("Image")?;
        Self::ensure(
            self.mode() == DisplayMode::Image,
            "Image button had no effect",
        )?;
        self.hold(2000)?;

        self.caption(
            8,
            TOTAL,
            "Click a trace to select it: its waveform and headers appear",
        );
        let p = self.at(0.50, 0.50)?;
        self.click_at(p)?;
        let picked = self
            .app()
            .controller()
            .selected_trace()
            .map(|s| s.trace_index)
            .ok_or("click did not select a trace")?;
        Self::ensure(
            self.app().controller().selected_trace_waveform().is_some(),
            "no waveform for the selected trace",
        )?;
        self.hold(2500)?;

        self.caption(9, TOTAL, "Arrow keys move the selection; Esc clears it");
        self.key(Key::ArrowRight)?;
        self.key(Key::ArrowRight)?;
        let moved = self
            .app()
            .controller()
            .selected_trace()
            .map(|s| s.trace_index);
        Self::ensure(
            moved == Some(picked + 2),
            format!("arrows moved to {moved:?}"),
        )?;
        self.key(Key::Escape)?;
        Self::ensure(
            self.app().controller().selected_trace().is_none(),
            "Esc did not clear the selection",
        )?;
        self.hold(1500)?;

        self.caption(10, TOTAL, "Back to the full section, then the X axis");
        self.click_label("Fit All")?;
        self.hold(1500)?;

        self.caption(11, TOTAL, "X axis from any trace header: pick CDP");
        self.click_label("X-Axis:")?;
        self.hold(900)?;
        self.click_label("CDP / Ensemble")?;
        Self::ensure(
            self.app().controller().viewport().horizontal_axis_key() == AxisHeaderKey::Cdp,
            "X-Axis combo did not switch to CDP",
        )?;
        self.hold(2200)?;

        self.caption(
            12,
            TOTAL,
            "Multiple X label rows (up to 5): each row can show a different trace header",
        );
        for expected in 1..=3 {
            self.click_label("+ Add label row")?;
            let rows = self.app().controller().extra_axis_keys().len();
            Self::ensure(
                rows == expected,
                format!("expected {expected} extra rows, got {rows}"),
            )?;
            self.hold(1500)?;
        }
        let keys = self.app().controller().extra_axis_keys().to_vec();
        Self::ensure(
            keys.iter()
                .enumerate()
                .all(|(i, k)| !keys[i + 1..].contains(k)),
            "extra rows must use distinct header keys",
        )?;
        self.hold(2500)?;
        self.click_label("Del")?;
        Self::ensure(
            self.app().controller().extra_axis_keys().len() == 2,
            "Del did not remove a row",
        )?;
        self.hold(2500)
    }
}
