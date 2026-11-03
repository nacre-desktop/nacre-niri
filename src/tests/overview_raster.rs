//! Raster coverage for final-coordinate overview clipping; `egl_` follows the Nix filter.

use std::time::Duration;

use niri_config::animations::{Curve, EasingParams, Kind};
use niri_config::Config;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::{Bind as _, Color32F, ExportMem as _};
use smithay::utils::{Scale, Transform};

use super::client::ClientId;
use super::Fixture;
use crate::layout::MonitorRenderElement;
use crate::niri::{Niri, OutputRenderElements};
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::{
    copy_framebuffer, create_texture, render_to_vec as render_pixels, RenderCtx, RenderTarget,
};

fn fixture() -> Fixture {
    let mut config = Config::parse_mem(
        r##"
        output "headless-1" { scale 1.5; }
        output "headless-2" { scale 1.0; }
        layout {
            border {
                on
                width 3
                active-gradient from="#ff9010" to="#10a0ff" angle=45 relative-to="workspace-view"
                inactive-gradient from="#3070ff" to="#e03090" angle=135 relative-to="workspace-view"
            }
            focus-ring { off; }
        }
    "##,
    )
    .unwrap();
    config.layout.collections.enabled = true;
    config.overview.workspace_shadow.off = false;
    let linear = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });
    config.animations.overview_open_close.0.kind = linear;
    config.animations.window_movement.0.kind = linear;
    config.animations.horizontal_view_movement.0.kind = linear;
    let mut fixture = Fixture::with_config(config);
    fixture
        .niri_state()
        .backend
        .headless()
        .add_renderer()
        .unwrap();
    fixture.add_output(1, (960, 600));
    fixture.add_output(2, (1280, 720));
    fixture
}

fn create_window(fixture: &mut Fixture, client: ClientId, column: u32) {
    let surface = {
        let window = fixture.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    fixture.roundtrip(client);
    let window = fixture.client(client).window(&surface);
    let buffer = window.spbm.create_u32_rgba_buffer(
        u32::MAX / (column + 2),
        u32::MAX / 4,
        u32::MAX / 2,
        u32::MAX,
        &window.qh,
        (),
    );
    window.surface.attach(Some(&buffer), 0, 0);
    window.set_size(520, 380);
    window.ack_last_and_commit();
    fixture.double_roundtrip(client);
}

fn set_time(niri: &mut Niri, millis: u64) {
    let now = niri.clock.now();
    niri.clock.set_unadjusted(now);
    let _ = niri.clock.now();
    niri.clock.set_unadjusted(Duration::ZERO);
    niri.clock.set_rate(1.0);
    let _ = niri.clock.now();
    niri.clock.set_unadjusted(Duration::from_millis(millis));
    let _ = niri.clock.now();
    niri.clock.set_rate(0.0);
}

struct RasterOutput {
    texture: GlesTexture,
    damage: OutputDamageTracker,
    initialized: bool,
    saw_clip: bool,
    saw_partial_damage: bool,
}

impl RasterOutput {
    fn new(fixture: &mut Fixture, index: u8) -> Self {
        let output = fixture.niri_output(index);
        let texture = fixture
            .niri_state()
            .backend
            .with_primary_renderer(|renderer| {
                assert!(
                    BorderRenderElement::has_shader(renderer),
                    "border shader unavailable"
                );
                create_texture(
                    renderer,
                    output.current_mode().unwrap().size,
                    Fourcc::Abgr8888,
                )
                .unwrap()
            })
            .unwrap();
        Self {
            texture,
            damage: OutputDamageTracker::from_output(&output),
            initialized: false,
            saw_clip: false,
            saw_partial_damage: false,
        }
    }

    fn compare(&mut self, fixture: &mut Fixture, index: u8, step: &str) {
        let output = fixture.niri_output(index);
        fixture.niri().update_render_elements(Some(&output));
        let state = &mut fixture.state.server.state;
        let niri = &state.niri;
        state
            .backend
            .with_primary_renderer(|renderer| {
                let elements = niri.render_to_vec(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    &output,
                    false,
                );
                self.saw_clip |= elements.iter().any(|element| {
                    matches!(
                        element,
                        OutputRenderElements::Monitor(MonitorRenderElement::ViewportClipped(_))
                    )
                });
                let size = output.current_mode().unwrap().size;
                let mut target = renderer.bind(&mut self.texture).unwrap();
                {
                    let age = usize::from(self.initialized);
                    let result = self
                        .damage
                        .render_output(renderer, &mut target, age, &elements, Color32F::TRANSPARENT)
                        .unwrap();
                    if let Some(damage) = result.damage {
                        let area: i64 = damage
                            .iter()
                            .map(|rect| i64::from(rect.size.w) * i64::from(rect.size.h))
                            .sum();
                        self.saw_partial_damage |=
                            area > 0 && area < i64::from(size.w) * i64::from(size.h);
                    }
                }
                self.initialized = true;
                let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
                let actual = renderer.map_texture(&mapping).unwrap().to_vec();
                drop(target);
                let expected = render_pixels(
                    renderer,
                    size,
                    Scale::from(output.current_scale().fractional_scale()),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.iter().rev(),
                )
                .unwrap();
                let mismatches = actual.iter().zip(&expected).filter(|(a, b)| a != b).count();
                assert_eq!(actual.len(), expected.len());
                assert_eq!(
                    mismatches, 0,
                    "partial repaint differs from full repaint: output {index}, {step}"
                );

                // The exact same source elements must retain both pixels and damage state.
                let mut target = renderer.bind(&mut self.texture).unwrap();
                let steady = self
                    .damage
                    .render_output(renderer, &mut target, 1, &elements, Color32F::TRANSPARENT)
                    .unwrap();
                assert!(
                    steady.damage.is_none_or(|damage| damage.is_empty()),
                    "unchanged frame damaged output {index}, {step}"
                );
            })
            .unwrap();
    }
}

#[test]
fn egl_overview_native_clip_partial_repaint_matches_full_raster_at_both_scales() {
    let mut fixture = fixture();
    let client = fixture.add_client();
    for index in [1, 2] {
        fixture.niri_focus_output(index);
        for workspace in 0..2 {
            for column in 0..3 {
                create_window(&mut fixture, client, column);
            }
            if workspace == 0 {
                fixture.niri().layout.switch_workspace_down();
            }
        }
    }
    fixture.niri_complete_animations();
    set_time(fixture.niri(), 0);
    let mut outputs = [
        RasterOutput::new(&mut fixture, 1),
        RasterOutput::new(&mut fixture, 2),
    ];
    fixture.niri().layout.open_overview();
    for millis in [0, 137, 333, 587, 811, 1000] {
        set_time(fixture.niri(), millis);
        fixture.niri().advance_animations();
        for (idx, output) in outputs.iter_mut().enumerate() {
            output.compare(&mut fixture, idx as u8 + 1, &format!("opening {millis}ms"));
        }
    }
    for index in [1, 2] {
        fixture.niri_focus_output(index);
        fixture.niri().layout.focus_column_first();
    }
    for millis in [1100, 1350, 1700, 2000] {
        set_time(fixture.niri(), millis);
        fixture.niri().advance_animations();
        for (idx, output) in outputs.iter_mut().enumerate() {
            output.compare(&mut fixture, idx as u8 + 1, &format!("camera {millis}ms"));
        }
    }
    fixture.niri().layout.close_overview();
    for millis in [2000, 2149, 2411, 2783, 3000] {
        set_time(fixture.niri(), millis);
        fixture.niri().advance_animations();
        for (idx, output) in outputs.iter_mut().enumerate() {
            output.compare(&mut fixture, idx as u8 + 1, &format!("closing {millis}ms"));
        }
    }
    for (idx, output) in outputs.iter().enumerate() {
        assert!(
            output.saw_clip,
            "output {} never exercised native clipping",
            idx + 1
        );
        assert!(
            output.saw_partial_damage,
            "output {} never exercised a partial repaint",
            idx + 1
        );
    }
}
