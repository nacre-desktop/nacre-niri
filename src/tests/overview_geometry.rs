//! The `egl_` test prefix follows the EGL sandbox filter in flake.nix.

use std::time::Duration;

use niri_config::animations::{Curve, EasingParams, Kind};
use niri_config::Config;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::Element as _;
use smithay::utils::{Rectangle, Scale, Transform};

use super::Fixture;
use crate::niri::{Niri, OutputRenderElements};
use crate::render_helpers::{render_to_vec as render_pixels, RenderCtx, RenderTarget};

fn fixture() -> Fixture {
    fixture_with_workspace_shadow(true)
}

fn fixture_with_workspace_shadow(off: bool) -> Fixture {
    let mut config = Config::parse_mem(
        "output \"headless-1\" { scale 1.5; }\noutput \"headless-2\" { scale 1.0; }",
    )
    .unwrap();
    config.layout.collections.enabled = true;
    config.overview.workspace_shadow.off = off;
    config.animations.overview_open_close.0.kind = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });
    let mut fixture = Fixture::with_config(config);
    fixture
        .niri_state()
        .backend
        .headless()
        .add_renderer()
        .unwrap();
    fixture.add_output(1, (2880, 1800));
    fixture.add_output(2, (3440, 1440));
    fixture
}

fn create_window(
    fixture: &mut Fixture,
    client: super::client::ClientId,
    w: u16,
    h: u16,
) -> wayland_client::protocol::wl_surface::WlSurface {
    let surface = {
        let window = fixture.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    fixture.roundtrip(client);
    let window = fixture.client(client).window(&surface);
    let buffer = window.spbm.create_u32_rgba_buffer(
        u32::MAX / 4,
        u32::MAX / 2,
        u32::MAX,
        u32::MAX,
        &window.qh,
        (),
    );
    window.surface.attach(Some(&buffer), 0, 0);
    window.set_size(w, h);
    window.ack_last_and_commit();
    fixture.double_roundtrip(client);
    surface
}

fn output_elements(
    fixture: &mut Fixture,
    output_idx: u8,
) -> (
    Vec<OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>>,
    f64,
) {
    output_elements_for_target(fixture, output_idx, RenderTarget::Output)
}

fn output_elements_for_target(
    fixture: &mut Fixture,
    output_idx: u8,
    target: RenderTarget,
) -> (
    Vec<OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>>,
    f64,
) {
    let output = fixture.niri_output(output_idx);
    fixture.niri().update_render_elements(Some(&output));
    let scale = output.current_scale().fractional_scale();
    let state = &mut fixture.state.server.state;
    let niri = &state.niri;
    let elements = state
        .backend
        .with_primary_renderer(|renderer| {
            niri.render_to_vec(
                RenderCtx {
                    renderer,
                    target,
                    xray: None,
                },
                &output,
                false,
            )
        })
        .expect("headless EGL renderer");
    (elements, scale)
}

fn damage_tracker(fixture: &Fixture, output_idx: u8) -> OutputDamageTracker {
    let output = fixture.niri_output(output_idx);
    OutputDamageTracker::new(
        output.current_mode().unwrap().size,
        output.current_scale().fractional_scale(),
        output.current_transform(),
    )
}

fn cached_preview_bounds(
    elements: &[OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>],
    scale: f64,
) -> Vec<Rectangle<i32, smithay::utils::Physical>> {
    elements
        .iter()
        .filter_map(|element| match element {
            OutputRenderElements::CollectionOverview(
                crate::layout::CollectionOverviewRenderElement::CachedPreview(preview),
            ) => Some(preview.geometry(Scale::from(scale))),
            _ => None,
        })
        .collect()
}

fn damage_covers_preview(
    damage: &[Rectangle<i32, smithay::utils::Physical>],
    rects: &[Rectangle<i32, smithay::utils::Physical>],
) -> bool {
    rects
        .iter()
        .any(|rect| rect.subtract_rects(damage.iter().copied()).is_empty())
}

fn assert_red_preview_pixels(
    fixture: &mut Fixture,
    output_idx: u8,
    elements: &[OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>],
    previews: &[Rectangle<i32, smithay::utils::Physical>],
) {
    let output = fixture.niri_output(output_idx);
    let size = output.current_mode().unwrap().size;
    let scale = Scale::from(output.current_scale().fractional_scale());
    let pixels = fixture
        .niri_state()
        .backend
        .with_primary_renderer(|renderer| {
            render_pixels(
                renderer,
                size,
                scale,
                Transform::Normal,
                Fourcc::Abgr8888,
                elements.iter().rev(),
            )
            .unwrap()
        })
        .unwrap();
    assert!(
        previews.iter().any(|rect| {
            (rect.loc.y..rect.loc.y + rect.size.h).any(|y| {
                (rect.loc.x..rect.loc.x + rect.size.w).any(|x| {
                    let offset = (y as usize * size.w as usize + x as usize) * 4;
                    let pixel = &pixels[offset..offset + 4];
                    pixel[0] > 240 && pixel[1] < 10 && pixel[2] < 10 && pixel[3] > 240
                })
            })
        }),
        "output {output_idx} did not draw the updated hidden window inside its preview"
    );
}

fn set_time(niri: &mut Niri, time: Duration) {
    let now = niri.clock.now();
    niri.clock.set_unadjusted(now);
    let _ = niri.clock.now();
    niri.clock.set_unadjusted(Duration::ZERO);
    niri.clock.set_rate(1.0);
    let _ = niri.clock.now();
    niri.clock.set_unadjusted(time);
    let _ = niri.clock.now();
    niri.clock.set_rate(0.0);
}

fn assert_unique_ids(
    elements: &[OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>],
) {
    let mut seen: Vec<smithay::backend::renderer::element::Id> = Vec::new();
    for element in elements {
        let id = element.id();
        assert!(
            !seen.iter().any(|seen_id| seen_id == id),
            "duplicate render element ID in one output frame"
        );
        seen.push(id.clone());
    }
}

fn element_signature(
    elements: &[OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>],
) -> Vec<(
    smithay::backend::renderer::element::Id,
    smithay::backend::renderer::utils::CommitCounter,
)> {
    elements
        .iter()
        .map(|element| (element.id().clone(), element.current_commit()))
        .collect()
}

fn native_content_ids(
    elements: &[OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>],
) -> Vec<smithay::backend::renderer::element::Id> {
    elements
        .iter()
        .filter_map(|element| {
            matches!(element, OutputRenderElements::Monitor(_)).then(|| element.id().clone())
        })
        .collect()
}

fn overview_content_ids(
    elements: &[OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>],
) -> Vec<smithay::backend::renderer::element::Id> {
    elements
        .iter()
        .filter_map(|element| match element {
            OutputRenderElements::Monitor(_) => Some(element.id().clone()),
            OutputRenderElements::CollectionOverview(
                crate::layout::CollectionOverviewRenderElement::Workspace(_),
            ) => Some(element.id().clone()),
            _ => None,
        })
        .collect()
}

fn contains(
    outer: Rectangle<i32, smithay::utils::Physical>,
    inner: Rectangle<i32, smithay::utils::Physical>,
) -> bool {
    outer.loc.x <= inner.loc.x
        && outer.loc.y <= inner.loc.y
        && outer.loc.x + outer.size.w >= inner.loc.x + inner.size.w
        && outer.loc.y + outer.size.h >= inner.loc.y + inner.size.h
}

#[test]
fn egl_overview_emits_consistent_workspace_and_window_geometry_on_both_outputs() {
    let mut fixture = fixture();
    let client = fixture.add_client();
    create_window(&mut fixture, client, 800, 600);
    create_window(&mut fixture, client, 800, 600);
    create_window(&mut fixture, client, 800, 600);
    fixture.niri().layout.switch_workspace_down();
    create_window(&mut fixture, client, 900, 700);
    let output2 = fixture.niri_output(2);
    fixture.niri().layout.focus_output(&output2);
    create_window(&mut fixture, client, 1200, 800);
    create_window(&mut fixture, client, 1200, 800);
    create_window(&mut fixture, client, 1200, 800);

    // Add a detached collection so both outputs render side-column previews while their
    // own shown collections continue through the normal monitor render path.
    let output1 = fixture.niri_output(1);
    fixture.niri().layout.focus_output(&output1);
    let output1_collection = fixture
        .niri()
        .layout
        .collections()
        .into_iter()
        .find(|collection| collection.output.as_deref() == Some("headless-1"))
        .unwrap()
        .id;
    let hidden_collection = fixture.niri().layout.create_collection(None);
    let hidden_surface = create_window(&mut fixture, client, 700, 500);
    assert_ne!(hidden_collection, output1_collection);
    fixture
        .niri()
        .layout
        .focus_collection(&niri_config::WorkspaceReference::Id(output1_collection));
    fixture.niri().layout.focus_output(&output1);
    fixture.niri_complete_animations();
    set_time(fixture.niri(), Duration::ZERO);
    let mut shown_content_ids = [Vec::new(), Vec::new()];
    for output_idx in [1, 2] {
        let (elements, _) = output_elements(&mut fixture, output_idx);
        shown_content_ids[usize::from(output_idx - 1)] = native_content_ids(&elements);
        assert!(
            !shown_content_ids[usize::from(output_idx - 1)].is_empty(),
            "output {output_idx} fixture has no native window elements"
        );
    }
    fixture.niri().layout.open_overview();
    let mut damage_trackers = [damage_tracker(&fixture, 1), damage_tracker(&fixture, 2)];

    for millis in [0, 250, 500, 750, 1000] {
        set_time(fixture.niri(), Duration::from_millis(millis));
        fixture.niri().advance_animations();
        let expected_zoom = 1. - 0.5 * millis as f64 / 1000.;
        assert!((fixture.niri().layout.overview_zoom() - expected_zoom).abs() < 1e-9);
        for output_idx in [1, 2] {
            let (elements, scale) = output_elements(&mut fixture, output_idx);
            assert_eq!(scale, if output_idx == 1 { 1.5 } else { 1.0 });
            let output = fixture.niri_output(output_idx);
            let shown_workspace_count = fixture
                .niri()
                .layout
                .monitor_for_output(&output)
                .unwrap()
                .workspaces_with_render_geo()
                .count();
            let shown_background_ids: Vec<_> = fixture
                .niri()
                .layout
                .monitor_for_output(&output)
                .unwrap()
                .workspaces_with_render_geo()
                .map(|(workspace, _)| workspace.render_background().id().clone())
                .collect();
            assert_unique_ids(&elements);
            assert_eq!(
                elements
                    .iter()
                    .filter(|element| matches!(
                        element,
                        OutputRenderElements::CollectionOverview(
                            crate::layout::CollectionOverviewRenderElement::Label(_)
                        )
                    ))
                    .count(),
                if millis == 0 { 1 } else { 2 },
                "each output has its own shown collection and one hidden side column"
            );
            let signature = element_signature(&elements);
            let cached = fixture.niri().layout.collection_overview_cache_counts();
            let tracker_idx = usize::from(output_idx - 1);
            damage_trackers[tracker_idx]
                .damage_output(1, &elements)
                .unwrap();
            let (steady, _) = output_elements(&mut fixture, output_idx);
            assert_eq!(
                signature,
                element_signature(&steady),
                "steady overview frame changed element IDs or commits on output {output_idx}"
            );
            let foreign_shown_ids = &shown_content_ids[usize::from(2 - output_idx)];
            let current_content_ids = overview_content_ids(&elements);
            assert!(
                foreign_shown_ids
                    .iter()
                    .all(|id| !current_content_ids.contains(id)),
                "output {output_idx} includes window elements from the other connected output's shown collection"
            );
            assert_eq!(
                cached,
                fixture
                    .niri()
                    .layout
                    .collection_overview_cache_counts(),
                "steady overview frame prepared or built a hidden preview again on output {output_idx}"
            );
            let (steady_damage, _) = damage_trackers[tracker_idx]
                .damage_output(1, &steady)
                .unwrap();
            assert!(
                steady_damage.is_none_or(|damage| damage.is_empty()),
                "unchanged overview frame produced damage on output {output_idx}: {steady_damage:?}"
            );

            let mut frames = Vec::new();
            let mut contents = Vec::new();
            let mut native_frames = Vec::new();
            let mut native_windows = Vec::new();
            for element in &elements {
                if let OutputRenderElements::CollectionOverview(preview) = element {
                    match preview {
                        crate::layout::CollectionOverviewRenderElement::Background(background) => {
                            frames.push(background.geometry(Scale::from(scale)));
                        }
                        crate::layout::CollectionOverviewRenderElement::Workspace(window) => {
                            contents.push(window.geometry(Scale::from(scale)));
                        }
                        crate::layout::CollectionOverviewRenderElement::CachedPreview(preview) => {
                            let geometry = preview.geometry(Scale::from(scale));
                            frames.push(geometry);
                            contents.push(geometry);
                        }
                        crate::layout::CollectionOverviewRenderElement::Label(_)
                        | crate::layout::CollectionOverviewRenderElement::Shadow(_) => {}
                    }
                } else {
                    match element {
                        OutputRenderElements::RelocatedColor(background) => {
                            let geometry = background.geometry(Scale::from(scale));
                            if shown_background_ids.contains(background.id()) {
                                native_frames.push(geometry);
                            }
                        }
                        OutputRenderElements::Monitor(window) => {
                            let bounds = Rectangle::from_size(output.current_mode().unwrap().size);
                            if let Some(visible) =
                                window.geometry(Scale::from(scale)).intersection(bounds)
                            {
                                native_windows.push(visible);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if millis > 0 {
                assert!(
                    !frames.is_empty(),
                    "no overview workspace backgrounds on output {output_idx}"
                );
                assert!(
                    !contents.is_empty(),
                    "no overview window previews on output {output_idx}"
                );
            }
            assert_eq!(native_frames.len(), shown_workspace_count);
            assert!(
                !native_windows.is_empty(),
                "no native shown-workspace windows on output {output_idx}"
            );
            for window in native_windows {
                assert!(
                    native_frames.iter().any(|frame| contains(*frame, window)),
                    "output {output_idx} scale {scale} progress {millis}ms: native window {window:?} is outside every shown workspace frame {native_frames:?}"
                );
            }
            for content in contents {
                assert!(
                    frames.iter().any(|frame| contains(*frame, content)),
                    "output {output_idx} scale {scale} progress {millis}ms: preview {content:?} is outside every workspace frame {frames:?}"
                );
            }
        }
    }

    // Commit fresh content to the hidden collection while its preview is cached. The
    // hidden workspace should be prepared and its GLES snapshot rebuilt once per output.
    let mut old_preview_bounds = [Vec::new(), Vec::new()];
    for output_idx in [1, 2] {
        let idx = usize::from(output_idx - 1);
        let (elements, scale) = output_elements(&mut fixture, output_idx);
        old_preview_bounds[idx] = cached_preview_bounds(&elements, scale);
        damage_trackers[idx].damage_output(1, &elements).unwrap();
    }
    let before_commit = fixture.niri().layout.collection_overview_cache_counts();
    let hidden_window = fixture.client(client).window(&hidden_surface);
    let buffer =
        hidden_window
            .spbm
            .create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &hidden_window.qh, ());
    hidden_window.surface.attach(Some(&buffer), 0, 0);
    hidden_window.commit();
    fixture.double_roundtrip(client);
    for output_idx in [1, 2] {
        let idx = usize::from(output_idx - 1);
        let (elements, _) = output_elements(&mut fixture, output_idx);
        let (damage, _) = damage_trackers[idx].damage_output(1, &elements).unwrap();
        assert!(
            damage.is_some_and(|damage| damage_covers_preview(damage, &old_preview_bounds[idx]))
        );
        assert_red_preview_pixels(
            &mut fixture,
            output_idx,
            &elements,
            &old_preview_bounds[idx],
        );
    }
    let after_commit = fixture.niri().layout.collection_overview_cache_counts();
    assert!(after_commit.0 > before_commit.0);
    assert!(after_commit.1 > before_commit.1);
    let _ = output_elements(&mut fixture, 1);
    let _ = output_elements(&mut fixture, 2);
    assert_eq!(
        after_commit,
        fixture.niri().layout.collection_overview_cache_counts(),
        "hidden preview was prepared or rebuilt again without a source change"
    );

    // Moving the cached collection onto output 1 changes the side-column contents on
    // both outputs. Damage must cover both the old and new preview bounds.
    fixture
        .niri()
        .layout
        .focus_collection(&niri_config::WorkspaceReference::Id(hidden_collection));
    for output_idx in [1, 2] {
        let idx = usize::from(output_idx - 1);
        let (elements, scale) = output_elements(&mut fixture, output_idx);
        let new_bounds = cached_preview_bounds(&elements, scale);
        let (damage, _) = damage_trackers[idx].damage_output(1, &elements).unwrap();
        let damage = damage.expect("collection move should damage the output");
        assert!(damage_covers_preview(&damage, &old_preview_bounds[idx]));
        assert!(!new_bounds.is_empty());
        assert!(damage_covers_preview(&damage, &new_bounds));
    }
}

#[test]
fn egl_overview_shadow_bounds_contain_hidden_workspace_preview_on_both_scales() {
    let mut fixture = fixture_with_workspace_shadow(false);
    let client = fixture.add_client();
    create_window(&mut fixture, client, 800, 600);
    let output2 = fixture.niri_output(2);
    fixture.niri().layout.focus_output(&output2);
    create_window(&mut fixture, client, 1200, 800);
    let output1 = fixture.niri_output(1);
    fixture.niri().layout.focus_output(&output1);
    let shown = fixture
        .niri()
        .layout
        .collections()
        .into_iter()
        .find(|collection| collection.output.as_deref() == Some("headless-1"))
        .unwrap()
        .id;
    let hidden_collection = fixture.niri().layout.create_collection(None);
    create_window(&mut fixture, client, 700, 500);
    fixture
        .niri()
        .layout
        .focus_collection(&niri_config::WorkspaceReference::Id(shown));
    fixture.niri_complete_animations();
    set_time(fixture.niri(), Duration::ZERO);
    fixture.niri().layout.open_overview();

    for millis in [0, 250, 500, 750, 1000] {
        set_time(fixture.niri(), Duration::from_millis(millis));
        fixture.niri().advance_animations();
        for output_idx in [1, 2] {
            let (elements, scale) = output_elements(&mut fixture, output_idx);
            let mut previews = Vec::new();
            let mut shadows = Vec::new();
            for element in &elements {
                if let OutputRenderElements::CollectionOverview(preview) = element {
                    match preview {
                        crate::layout::CollectionOverviewRenderElement::CachedPreview(frame) => {
                            previews.push(frame.geometry(Scale::from(scale)));
                        }
                        crate::layout::CollectionOverviewRenderElement::Shadow(shadow) => {
                            shadows
                                .push((shadow.id().clone(), shadow.geometry(Scale::from(scale))));
                        }
                        _ => {}
                    }
                }
            }
            if millis > 0 {
                assert!(!previews.is_empty());
                // A shadow can be split into several shader rectangles to exclude the
                // workspace interior. Group those rectangles by their source workspace.
                let state = &mut fixture.state.server.state;
                let niri = &state.niri;
                let shadow_ids: Vec<Vec<_>> = state
                    .backend
                    .with_primary_renderer(|renderer| {
                        niri.layout
                            .collection_workspaces(hidden_collection)
                            .map(|workspace| {
                                let mut ids = Vec::new();
                                workspace.render_shadow(renderer, &mut |element| {
                                    ids.push(element.id().clone())
                                });
                                ids
                            })
                            .collect()
                    })
                    .unwrap();
                let shadow_bounds: Vec<_> = shadow_ids
                    .iter()
                    .filter_map(|ids| {
                        shadows
                            .iter()
                            .filter(|(id, _)| ids.contains(id))
                            .map(|(_, geometry)| *geometry)
                            .reduce(|a, b| a.merge(b))
                    })
                    .collect();
                assert_eq!(previews.len(), shadow_bounds.len());
                for preview in previews {
                    assert!(shadow_bounds
                        .iter()
                        .any(|shadow| contains(*shadow, preview)));
                }
            }
        }
    }
}

#[test]
fn egl_many_hidden_previews_remain_cached_across_outputs_and_capture_targets() {
    let mut fixture = fixture();
    let client = fixture.add_client();
    let output1 = fixture.niri_output(1);
    fixture.niri().layout.focus_output(&output1);
    let shown = fixture
        .niri()
        .layout
        .collection_for_output(&output1)
        .unwrap();
    for _ in 0..6 {
        fixture.niri().layout.create_collection(None);
        create_window(&mut fixture, client, 700, 500);
        fixture
            .niri()
            .layout
            .focus_collection(&niri_config::WorkspaceReference::Id(shown));
    }
    fixture.niri_complete_animations();
    fixture.niri().layout.open_overview();
    fixture.niri_complete_animations();
    let targets = [
        RenderTarget::Output,
        RenderTarget::Screencast,
        RenderTarget::ScreenCapture,
    ];
    for target in targets {
        for output in [1, 2] {
            let _ = output_elements_for_target(&mut fixture, output, target);
        }
    }
    let warmed = fixture.niri().layout.collection_overview_cache_counts();
    // Six hidden stacks each retain a populated and trailing empty workspace: 72
    // visible snapshots across the two outputs and three capture targets.
    assert!(
        warmed.1 > 64,
        "fixture must exercise more than 64 cached snapshots: {warmed:?}"
    );
    for target in targets {
        for output in [2, 1] {
            let _ = output_elements_for_target(&mut fixture, output, target);
        }
    }
    assert_eq!(
        warmed,
        fixture.niri().layout.collection_overview_cache_counts()
    );
}
