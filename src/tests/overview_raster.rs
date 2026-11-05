//! Raster coverage for final-coordinate overview clipping; `egl_` follows the Nix filter.

use std::collections::HashSet;
use std::time::Duration;

use niri_config::animations::{Curve, EasingParams, Kind};
use niri_config::Config;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::{Bind as _, Color32F, ExportMem as _};

use super::client::ClientId;
use super::Fixture;
use crate::layout::MonitorRenderElement;
use crate::niri::{Niri, OutputRenderElements};
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::{copy_framebuffer, create_texture, RenderCtx, RenderTarget};

fn fixture(collections: bool) -> Fixture {
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
    config.layout.collections.enabled = collections;
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
    full_texture: GlesTexture,
    damage: OutputDamageTracker,
    initialized: bool,
    saw_clip: bool,
    saw_partial_damage: bool,
}

impl RasterOutput {
    fn new(fixture: &mut Fixture, index: u8) -> Self {
        let output = fixture.niri_output(index);
        let (texture, full_texture) = fixture
            .niri_state()
            .backend
            .with_primary_renderer(|renderer| {
                assert!(
                    BorderRenderElement::has_shader(renderer),
                    "border shader unavailable"
                );
                let size = output.current_mode().unwrap().size;
                (
                    create_texture(renderer, size, Fourcc::Abgr8888).unwrap(),
                    create_texture(renderer, size, Fourcc::Abgr8888).unwrap(),
                )
            })
            .unwrap();
        Self {
            texture,
            full_texture,
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
                        scale_override: None,
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
                // A fresh tracker and age zero force a full repaint. Use the same
                // opaque culling/scissor path as the incremental renderer: drawing
                // every element directly can round shader pixels differently even
                // on the first full frame, before incremental damage is involved.
                let mut full_target = renderer.bind(&mut self.full_texture).unwrap();
                OutputDamageTracker::from_output(&output)
                    .render_output(
                        renderer,
                        &mut full_target,
                        0,
                        &elements,
                        Color32F::TRANSPARENT,
                    )
                    .unwrap();
                let mapping = copy_framebuffer(renderer, &full_target, Fourcc::Abgr8888).unwrap();
                let expected = renderer.map_texture(&mapping).unwrap().to_vec();
                drop(full_target);
                let first_mismatch = actual
                    .iter()
                    .zip(&expected)
                    .enumerate()
                    .find(|(_, (actual, expected))| actual != expected)
                    .map(|(byte, (actual, expected))| {
                        let pixel = byte / 4;
                        (
                            pixel % size.w as usize,
                            pixel / size.w as usize,
                            byte % 4,
                            actual,
                            expected,
                        )
                    });
                let mismatches = actual.iter().zip(&expected).filter(|(a, b)| a != b).count();
                assert_eq!(actual.len(), expected.len());
                assert_eq!(
                    mismatches, 0,
                    "partial repaint differs from full repaint: output {index}, {step}; first differing (x, y, channel, actual, expected): {first_mismatch:?}"
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
    for with_side_columns in [false, true] {
        check_native_overview_raster(with_side_columns);
    }
}

fn check_native_overview_raster(with_side_columns: bool) {
    let mut fixture = fixture(true);
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
    if with_side_columns {
        let output = fixture.niri_output(2);
        let shown = fixture
            .niri()
            .layout
            .collection_for_output(&output)
            .unwrap();
        fixture
            .niri()
            .layout
            .create_collection(Some("Raster side desk".into()));
        create_window(&mut fixture, client, 0);
        fixture
            .niri()
            .layout
            .focus_collection(&niri_config::WorkspaceReference::Id(shown));
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
        assert_eq!(
            output.saw_clip,
            with_side_columns,
            "output {} clipping must depend on side columns",
            idx + 1,
        );
        assert!(
            output.saw_partial_damage,
            "output {} never exercised a partial repaint",
            idx + 1
        );
    }
}

#[test]
fn egl_scratchpad_overview_uses_cached_transformed_preview_with_and_without_collections() {
    for collections in [false, true] {
        for output_index in [1, 2] {
            let mut fixture = fixture(collections);
            fixture.niri_focus_output(output_index);
            let client = fixture.add_client();
            for column in 0..4 {
                create_window(&mut fixture, client, column);
            }
            let window = fixture.niri().layout.focus().unwrap().window.clone();
            fixture
                .niri()
                .layout
                .move_window_to_scratchpad(Some(&window));
            let workspace = fixture.niri().layout.active_workspace().unwrap();
            assert_eq!(workspace.windows().count(), 3);
            assert_eq!(workspace.scrolling().columns().count(), 3);
            assert!(workspace
                .windows()
                .all(|window| !workspace.is_floating(&window.window)));
            fixture.niri_complete_animations();
            // Opening must start at zero, rather than the fixture's completed-animation time.
            set_time(fixture.niri(), 0);
            let mut output = RasterOutput::new(&mut fixture, output_index);
            fixture.niri().layout.open_overview();

            for millis in [0, 137, 500, 1000] {
                set_time(fixture.niri(), millis);
                fixture.niri().advance_animations();
                output.compare(
                    &mut fixture,
                    output_index,
                    &format!("scratchpad {collections} {output_index} {millis}ms"),
                );
            }
            fixture.niri_complete_animations();
            assert!(fixture.niri().layout.is_overview_open());
            let existing_ids: HashSet<_> = fixture
                .niri()
                .layout
                .windows()
                .map(|(_, mapped)| mapped.id())
                .collect();
            create_window(&mut fixture, client, 4);
            let added = fixture
                .niri()
                .layout
                .windows()
                .find(|(_, mapped)| !existing_ids.contains(&mapped.id()))
                .unwrap()
                .1
                .window
                .clone();
            fixture
                .niri()
                .layout
                .move_window_to_scratchpad(Some(&added));
            assert!(fixture.niri().layout.is_window_hidden(&added));
            assert!(fixture.niri().layout.is_overview_open());
            assert_eq!(
                fixture
                    .niri()
                    .layout
                    .active_workspace()
                    .unwrap()
                    .windows()
                    .count(),
                3
            );
            output.compare(
                &mut fixture,
                output_index,
                &format!("scratchpad content change {collections} {output_index}"),
            );
            let (preparations, builds) = fixture.niri().layout.collection_overview_cache_counts();
            assert!(
                builds > 0,
                "scratchpad preview did not use the shared texture cache"
            );
            output.compare(&mut fixture, output_index, "steady scratchpad cache");
            let (steady_preparations, steady_builds) =
                fixture.niri().layout.collection_overview_cache_counts();
            assert_eq!(
                steady_builds, builds,
                "steady scratchpad frame rebuilt its texture"
            );
            assert_eq!(
                steady_preparations, preparations,
                "steady scratchpad frame re-prepared its workspace"
            );
            assert!(output.saw_partial_damage,
                "scratchpad preview never caused partial damage for collections={collections}, output={output_index}");
        }
    }
}
