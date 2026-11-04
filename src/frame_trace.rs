//! Opt-in CPU frame timings. No GPU readback or fence wait is introduced by tracing.

use std::cell::Cell;
use std::sync::OnceLock;
use std::time::Instant;

thread_local! {
    static FRAME: Cell<u64> = const { Cell::new(0) };
    static OVERVIEW: Cell<u64> = const { Cell::new(0) };
    static SCROLLED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("NACRE_NIRI_FRAME_TRACE").is_some_and(|v| v == "1"))
}

pub(crate) fn marker(event: &'static str) {
    if enabled() {
        info!(target: "nacre_niri_frame_trace", event, frame = FRAME.get(), overview = OVERVIEW.get());
    }
}

pub(crate) fn overview_open() {
    if enabled() {
        OVERVIEW.set(OVERVIEW.get().wrapping_add(1));
        SCROLLED.set(false);
        marker("overview_open");
    }
}

pub(crate) fn overview_scroll(axis: &'static str, ticks: i8) {
    if enabled() {
        let event = if SCROLLED.replace(true) {
            "overview_scroll"
        } else {
            "overview_first_scroll"
        };
        info!(target: "nacre_niri_frame_trace", event, axis, ticks, frame = FRAME.get(), overview = OVERVIEW.get());
    }
}

pub(crate) struct Timer {
    event: &'static str,
    start: Instant,
    output: Option<String>,
    minimum_us: u64,
}

pub(crate) fn span(event: &'static str) -> Option<Timer> {
    enabled().then(|| Timer {
        event,
        start: Instant::now(),
        output: None,
        minimum_us: 0,
    })
}

/// Surface traversal happens for every visible surface. Only log slow imports, including any
/// client-buffer import or driver wait performed inside the renderer's import call.
pub(crate) fn surface_import() -> Option<Timer> {
    span("surface_import_slow").map(|mut timer| {
        timer.minimum_us = 500;
        timer
    })
}

pub(crate) fn frame(output: &smithay::output::Output) -> Option<Timer> {
    if !enabled() {
        return None;
    }
    FRAME.set(FRAME.get().wrapping_add(1));
    Some(Timer {
        event: "frame",
        start: Instant::now(),
        output: Some(output.name()),
        minimum_us: 0,
    })
}

impl Drop for Timer {
    fn drop(&mut self) {
        let cpu_us = self.start.elapsed().as_micros() as u64;
        if cpu_us < self.minimum_us {
            return;
        }
        info!(
            target: "nacre_niri_frame_trace",
            event = self.event,
            frame = FRAME.get(),
            overview = OVERVIEW.get(),
            output = self.output.as_deref().unwrap_or(""),
            cpu_us,
        );
    }
}
