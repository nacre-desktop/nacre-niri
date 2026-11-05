//! Fidelity checks for hidden collection previews rendered at another output's scale.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Element as _;
use smithay::utils::{Scale, Transform};

use super::client::ClientId;
use super::Fixture;
use crate::layout::CollectionOverviewRenderElement;
use crate::niri::OutputRenderElements;
use crate::render_helpers::{render_to_vec as render_pixels, RenderCtx, RenderTarget};

const STRIPE_WIDTH: i32 = 64;
const STRIPE_STRIDE: i32 = 96;
const STRIPE_COLORS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]];
const MOVED_STRIPE_COLOR: [u8; 3] = [255, 0, 255];

fn fixture() -> Fixture {
    let mut config = niri_config::Config::parse_mem(
        "output \"headless-1\" { scale 1.5; }\noutput \"headless-2\" { scale 1.0; }",
    )
    .unwrap();
    config.layout.collections.enabled = true;
    config.animations.off = true;
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

fn create_pattern_window(
    fixture: &mut Fixture,
    client_id: ClientId,
) -> (
    wayland_client::protocol::wl_surface::WlSurface,
    Vec<(
        wayland_client::protocol::wl_surface::WlSurface,
        wayland_client::protocol::wl_subsurface::WlSubsurface,
        smithay::reexports::wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport,
    )>,
) {
    let parent = {
        let window = fixture.client(client_id).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    fixture.roundtrip(client_id);

    let (qh, spbm, parent, compositor, subcompositor, viewporter) = {
        let client = fixture.client(client_id);
        let window = client.window(&parent);
        (
            window.qh.clone(),
            window.spbm.clone(),
            window.surface.clone(),
            client.state.compositor.as_ref().unwrap().clone(),
            client.state.subcompositor.as_ref().unwrap().clone(),
            client.state.viewporter.as_ref().unwrap().clone(),
        )
    };
    let base = spbm.create_u32_rgba_buffer(16, 16, 16, u32::MAX, &qh, ());
    parent.attach(Some(&base), 0, 0);
    {
        let window = fixture.client(client_id).window(&parent);
        window.set_size(700, 500);
        window.ack_last_and_commit();
    }
    fixture.double_roundtrip(client_id);

    let mut children = Vec::new();
    for (index, color) in STRIPE_COLORS.iter().enumerate() {
        let child = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
        subsurface.set_position(80 + index as i32 * STRIPE_STRIDE, 72);
        let viewport = viewporter.get_viewport(&child, &qh, ());
        viewport.set_destination(STRIPE_WIDTH, 320);
        let channel = |value: u8| if value == 0 { 0 } else { u32::MAX };
        let buffer = spbm.create_u32_rgba_buffer(
            channel(color[0]),
            channel(color[1]),
            channel(color[2]),
            u32::MAX,
            &qh,
            (),
        );
        child.attach(Some(&buffer), 0, 0);
        child.commit();
        children.push((child, subsurface, viewport));
    }
    parent.commit();
    fixture.double_roundtrip(client_id);
    (parent, children)
}

fn classify(pixel: &[u8]) -> Option<usize> {
    STRIPE_COLORS.iter().position(|color| {
        pixel[0].abs_diff(color[0]) < 5
            && pixel[1].abs_diff(color[1]) < 5
            && pixel[2].abs_diff(color[2]) < 5
            && pixel[3] > 240
    })
}

fn row_runs(
    pixels: &[u8],
    width: usize,
    y: usize,
    x_start: usize,
    x_end: usize,
) -> Vec<(usize, usize, usize)> {
    let mut runs = Vec::new();
    let mut current: Option<(usize, usize, usize)> = None;
    for x in x_start..x_end {
        let offset = (y * width + x) * 4;
        let color = classify(&pixels[offset..offset + 4]);
        match (current, color) {
            (Some((start, _previous, color)), Some(next)) if color == next => {
                current = Some((start, x, color));
            }
            (Some(run), next) => {
                if run.1 - run.0 + 1 >= 6 {
                    runs.push(run);
                }
                current = next.map(|color| (x, x, color));
            }
            (None, Some(color)) => current = Some((x, x, color)),
            (None, None) => {}
        }
    }
    if let Some(run) = current {
        if run.1 - run.0 + 1 >= 6 {
            runs.push(run);
        }
    }
    runs
}

fn pattern_centers(
    pixels: &[u8],
    size: smithay::utils::Size<i32, smithay::utils::Physical>,
    bounds: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
) -> Option<Vec<(f64, f64)>> {
    let x_start = bounds.loc.x.max(0) as usize;
    let x_end = (bounds.loc.x + bounds.size.w).min(size.w).max(0) as usize;
    let y_start = bounds.loc.y.max(0) as usize;
    let y_end = (bounds.loc.y + bounds.size.h).min(size.h).max(0) as usize;
    for y in y_start..y_end {
        let runs = row_runs(pixels, size.w as usize, y, x_start, x_end);
        for sequence in runs.windows(4) {
            if sequence
                .iter()
                .enumerate()
                .all(|(index, run)| run.2 == index)
            {
                return Some(
                    sequence
                        .iter()
                        .map(|run| ((run.0 + run.1) as f64 / 2., y as f64))
                        .collect::<Vec<_>>(),
                );
            }
        }
    }
    None
}

fn assert_pattern_spacing(
    pixels: &[u8],
    size: smithay::utils::Size<i32, smithay::utils::Physical>,
    bounds: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
    expected: f64,
) {
    let centers = pattern_centers(pixels, size, bounds)
        .expect("four colored subsurface stripes visible in hidden preview");
    for pair in centers.windows(2) {
        let distance = pair[1].0 - pair[0].0;
        assert!(
            (distance - expected).abs() <= 3.,
            "preview stripe centers should follow target scale and zoom: expected {expected:.2}px, got {distance:.2}px"
        );
    }
}

fn color_present_in_preview(
    pixels: &[u8],
    size: smithay::utils::Size<i32, smithay::utils::Physical>,
    bounds: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
    color: [u8; 3],
) -> bool {
    (bounds.loc.y.max(0)..(bounds.loc.y + bounds.size.h).min(size.h)).any(|y| {
        (bounds.loc.x.max(0)..(bounds.loc.x + bounds.size.w).min(size.w)).any(|x| {
            let offset = (y as usize * size.w as usize + x as usize) * 4;
            let pixel = &pixels[offset..offset + 4];
            pixel[0].abs_diff(color[0]) < 5
                && pixel[1].abs_diff(color[1]) < 5
                && pixel[2].abs_diff(color[2]) < 5
                && pixel[3] > 240
        })
    })
}

fn render_target_preview(
    fixture: &mut Fixture,
    target_index: u8,
) -> (
    Vec<u8>,
    smithay::utils::Size<i32, smithay::utils::Physical>,
    Vec<smithay::utils::Rectangle<i32, smithay::utils::Physical>>,
) {
    let target_output = fixture.niri_output(target_index);
    fixture.niri().update_render_elements(Some(&target_output));
    let output_scale = target_output.current_scale().fractional_scale();
    let size = target_output.current_mode().unwrap().size;
    let elements = {
        let state = &mut fixture.state.server.state;
        let niri = &state.niri;
        state
            .backend
            .with_primary_renderer(|renderer| {
                niri.render_to_vec(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                        scale_override: None,
                    },
                    &target_output,
                    false,
                )
            })
            .unwrap()
    };
    let previews = elements
        .iter()
        .filter_map(|element| match element {
            OutputRenderElements::CollectionOverview(
                CollectionOverviewRenderElement::CachedPreview(preview),
            ) => Some(preview.geometry(Scale::from(output_scale))),
            _ => None,
        })
        .collect();
    let pixels = fixture
        .niri_state()
        .backend
        .with_primary_renderer(|renderer| {
            render_pixels(
                renderer,
                size,
                Scale::from(output_scale),
                Transform::Normal,
                Fourcc::Abgr8888,
                elements.iter().rev(),
            )
            .unwrap()
        })
        .unwrap();
    (pixels, size, previews)
}

fn matching_preview(
    pixels: &[u8],
    size: smithay::utils::Size<i32, smithay::utils::Physical>,
    previews: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
) -> smithay::utils::Rectangle<i32, smithay::utils::Physical> {
    previews
        .iter()
        .copied()
        .find(|preview| pattern_centers(pixels, size, *preview).is_some())
        .expect("cached preview containing the patterned workspace")
}

fn color_center(
    pixels: &[u8],
    size: smithay::utils::Size<i32, smithay::utils::Physical>,
    bounds: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
    color: [u8; 3],
) -> Option<(f64, f64)> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for y in bounds.loc.y.max(0)..(bounds.loc.y + bounds.size.h).min(size.h) {
        for x in bounds.loc.x.max(0)..(bounds.loc.x + bounds.size.w).min(size.w) {
            let offset = (y as usize * size.w as usize + x as usize) * 4;
            let pixel = &pixels[offset..offset + 4];
            if pixel[0].abs_diff(color[0]) < 5
                && pixel[1].abs_diff(color[1]) < 5
                && pixel[2].abs_diff(color[2]) < 5
                && pixel[3] > 240
            {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    (min_x <= max_x).then_some(((min_x + max_x) as f64 / 2., (min_y + max_y) as f64 / 2.))
}

fn color_near(
    pixels: &[u8],
    size: smithay::utils::Size<i32, smithay::utils::Physical>,
    bounds: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
    point: (f64, f64),
    color: [u8; 3],
) -> bool {
    let x = point.0.round() as i32;
    let y = point.1.round() as i32;
    (-2..=2).any(|dy| {
        (-2..=2).any(|dx| {
            let x = x + dx;
            let y = y + dy;
            if x < bounds.loc.x
                || y < bounds.loc.y
                || x >= bounds.loc.x + bounds.size.w
                || y >= bounds.loc.y + bounds.size.h
                || x < 0
                || y < 0
                || x >= size.w
                || y >= size.h
            {
                return false;
            }
            let offset = (y as usize * size.w as usize + x as usize) * 4;
            let pixel = &pixels[offset..offset + 4];
            pixel[0].abs_diff(color[0]) < 5
                && pixel[1].abs_diff(color[1]) < 5
                && pixel[2].abs_diff(color[2]) < 5
                && pixel[3] > 240
        })
    })
}

#[test]
fn egl_hidden_collection_preview_rebases_surface_positions_at_target_scale_and_zoom() {
    for (source_index, target_index) in [(1_u8, 2_u8), (2, 1)] {
        let mut fixture = fixture();
        let client = fixture.add_client();
        let source_output = fixture.niri_output(source_index);
        fixture.niri().layout.focus_output(&source_output);
        let source_name = source_output.name();
        let source_home_id = fixture
            .niri()
            .layout
            .collections()
            .into_iter()
            .find(|collection| collection.output.as_deref() == Some(source_name.as_str()))
            .unwrap()
            .id;
        let hidden_collection = fixture
            .niri()
            .layout
            .create_collection(Some("Preview fidelity".into()));
        fixture
            .niri()
            .layout
            .focus_collection(&niri_config::WorkspaceReference::Id(hidden_collection));
        let (parent, children) = create_pattern_window(&mut fixture, client);
        fixture
            .niri()
            .layout
            .focus_collection(&niri_config::WorkspaceReference::Id(source_home_id));
        fixture.niri_complete_animations();

        fixture.niri().layout.open_overview();
        fixture.niri_complete_animations();
        let (pixels, size, previews) = render_target_preview(&mut fixture, target_index);
        let preview = matching_preview(&pixels, size, &previews);
        // The preview may be cropped by the target output edge. Its visible element bounds then
        // report only the cropped width, so derive pixel spacing from the source logical
        // coordinates and the native overview zoom instead of the visible crop width.
        let preview_scale = fixture.niri().layout.overview_zoom()
            * fixture
                .niri_output(target_index)
                .current_scale()
                .fractional_scale();
        let expected_stride = STRIPE_STRIDE as f64 * preview_scale;
        assert_pattern_spacing(&pixels, size, preview, expected_stride);

        let built = fixture.niri().layout.collection_overview_cache_counts();
        let _ = render_target_preview(&mut fixture, target_index);
        assert_eq!(
            built,
            fixture.niri().layout.collection_overview_cache_counts(),
            "steady preview frames reuse the workspace snapshot and texture"
        );

        fixture.niri().layout.close_overview();
        fixture.niri_complete_animations();
        fixture.niri().layout.open_overview();
        fixture.niri_complete_animations();
        let (reopened_pixels, reopened_size, reopened_previews) =
            render_target_preview(&mut fixture, target_index);
        let reopened_preview =
            matching_preview(&reopened_pixels, reopened_size, &reopened_previews);
        let after_reopen = fixture.niri().layout.collection_overview_cache_counts();
        assert!(
            after_reopen.1 > built.1,
            "opening overview refreshes hidden snapshots"
        );
        assert_pattern_spacing(
            &reopened_pixels,
            reopened_size,
            reopened_preview,
            expected_stride,
        );

        let moved_output = fixture.niri_output(target_index);
        let moved_name = moved_output.name();
        let moved_home_id = fixture
            .niri()
            .layout
            .collections()
            .into_iter()
            .find(|collection| collection.output.as_deref() == Some(moved_name.as_str()))
            .unwrap()
            .id;
        fixture.niri().layout.focus_output(&moved_output);
        fixture
            .niri()
            .layout
            .focus_collection(&niri_config::WorkspaceReference::Id(hidden_collection));
        fixture
            .niri()
            .layout
            .focus_collection(&niri_config::WorkspaceReference::Id(moved_home_id));
        fixture.niri_complete_animations();
        let (moved_pixels, moved_size, moved_previews) =
            render_target_preview(&mut fixture, source_index);
        let moved_preview = matching_preview(&moved_pixels, moved_size, &moved_previews);
        let after_scale_change = fixture.niri().layout.collection_overview_cache_counts();
        assert!(
            after_scale_change.1 > after_reopen.1,
            "moving a desk to another scale and size rebuilds its hidden preview"
        );
        let moved_preview_scale = fixture.niri().layout.overview_zoom()
            * fixture
                .niri_output(source_index)
                .current_scale()
                .fractional_scale();
        assert_pattern_spacing(
            &moved_pixels,
            moved_size,
            moved_preview,
            STRIPE_STRIDE as f64 * moved_preview_scale,
        );
        let old_stripe_center =
            color_center(&moved_pixels, moved_size, moved_preview, STRIPE_COLORS[0])
                .expect("original stripe is visible in the selected preview");
        let expected_moved_center = (
            old_stripe_center.0 + (520. - 80.) * moved_preview_scale,
            old_stripe_center.1,
        );

        let child = children[0].0.clone();
        children[0].1.set_position(520, 72);
        let (qh, spbm) = {
            let client = fixture.client(client);
            (
                client.qh.clone(),
                client.state.spbm.as_ref().unwrap().clone(),
            )
        };
        let changed = spbm.create_u32_rgba_buffer(u32::MAX, 0, u32::MAX, u32::MAX, &qh, ());
        child.attach(Some(&changed), 0, 0);
        child.commit();
        parent.commit();
        fixture.double_roundtrip(client);
        let (changed_pixels, changed_size, changed_previews) =
            render_target_preview(&mut fixture, source_index);
        let after_commit = fixture.niri().layout.collection_overview_cache_counts();
        assert!(
            after_commit.1 > after_scale_change.1,
            "a committed surface rebuilds its preview texture"
        );
        let changed_preview = changed_previews
            .iter()
            .copied()
            .find(|preview| {
                color_center(&changed_pixels, changed_size, *preview, MOVED_STRIPE_COLOR).is_some()
            })
            .expect("committed moved stripe appears in a cached preview");
        let changed_center = color_center(
            &changed_pixels,
            changed_size,
            changed_preview,
            MOVED_STRIPE_COLOR,
        )
        .unwrap();
        assert!(
            (changed_center.0 - expected_moved_center.0).abs() <= 2.
                && (changed_center.1 - expected_moved_center.1).abs() <= 2.,
            "moved color center {changed_center:?} should match {expected_moved_center:?}"
        );
        assert!(
            !color_near(
                &changed_pixels,
                changed_size,
                changed_preview,
                old_stripe_center,
                STRIPE_COLORS[0]
            ),
            "the previous stripe position no longer shows its old color"
        );

        child.attach(None, 0, 0);
        child.commit();
        parent.commit();
        fixture.double_roundtrip(client);
        let (unmapped_pixels, unmapped_size, _) = render_target_preview(&mut fixture, source_index);
        let after_unmap = fixture.niri().layout.collection_overview_cache_counts();
        assert!(
            after_unmap.1 > after_commit.1,
            "unmapping a surface rebuilds its preview"
        );
        assert!(
            !color_present_in_preview(
                &unmapped_pixels,
                unmapped_size,
                moved_preview,
                MOVED_STRIPE_COLOR
            ),
            "an uncommitted old frame is not retained in the hidden preview"
        );
    }
}
