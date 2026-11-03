//! Exercise the actual atlas render wrappers with a surfaceless GPU renderer.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id};
use smithay::desktop::WindowSurfaceType;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};

use super::client::bead_layer_motion::v1::client::bead_layer_motion_v1::{
    self, Curve, OpacityMode as WireOpacityMode,
};
use super::client::LayerConfigureProps;
use super::Fixture;
use crate::layer::motion::{MotionPose, OpacityMode};
use crate::protocols::layer_motion::{LayerMotionRuntime, MotionPlane};
use crate::render_helpers::damage::ExtraDamage;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::{self, RenderCtx, RenderTarget};

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rectangle<f64, Logical> {
    Rectangle::new(Point::from((x, y)), Size::from((width, height)))
}

fn plane(id: u32, source: Rectangle<f64, Logical>, pose: MotionPose) -> MotionPlane {
    let mut damage = ExtraDamage::new();
    damage.damage_all();
    MotionPlane {
        id,
        render_id: Id::new(),
        damage,
        source,
        from: pose,
        to: pose,
        opacity_mode: OpacityMode::Linear,
    }
}

fn ack_new_layer_configure(layer: &super::client::LayerSurface, last_acked: &mut Option<u32>) {
    let Some((serial, _)) = layer.configures_received.last() else {
        return;
    };
    if *last_acked != Some(*serial) {
        layer.layer_surface.ack_configure(*serial);
        *last_acked = Some(*serial);
    }
}

#[test]
fn gpu_planes_share_a_buffer_but_preserve_crop_opacity_damage_and_capture_pixels() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.niri_state()
        .backend
        .headless()
        .add_renderer()
        .expect("surfaceless GLES renderer for layer-motion pixels");
    let client_id = f.add_client();
    let layer = f
        .client(client_id)
        .create_layer(None, Layer::Top, "motion-render");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((96, 72)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(client_id);
    let layer = f.client(client_id).layer(&surface);
    // One immutable, opaque red texture; both planes sample that same surface.
    let buffer = layer
        .spbm
        .create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &layer.qh, ());
    layer.surface.attach(Some(&buffer), 0, 0);
    layer.set_size(96, 72);
    layer.ack_last_and_commit();
    f.double_roundtrip(client_id);

    let first = MotionPose {
        destination: rect(8., 4., 32., 24.),
        clip: rect(16., 8., 16., 16.),
        opacity: 1.,
    };
    let second = MotionPose {
        destination: rect(48., 16., 16., 32.),
        clip: rect(0., 0., 96., 72.),
        opacity: 0.5,
    };
    let runtime = Arc::new(Mutex::new(LayerMotionRuntime {
        planes: vec![
            plane(0, rect(8., 0., 32., 24.), first),
            // Nonuniform resize ensures source crops are interpreted in logical
            // viewport coordinates rather than raw one-pixel buffer coordinates.
            plane(1, rect(0., 0., 32., 16.), second),
        ],
        current: vec![(0, first), (1, second)],
        active: None,
    }));
    let mapped = f.niri().mapped_layer_surfaces.values_mut().next().unwrap();
    mapped.update_sizes(Size::from((96., 72.)), 1.5);
    mapped.set_motion_runtime(runtime.clone());
    assert_eq!(
        mapped.motion_input_point(Point::from((52., 20.))),
        Some(Some(Point::from((8., 2.))))
    );
    assert_eq!(
        mapped.motion_input_point(Point::from((12., 6.))),
        Some(None)
    );

    let mut previous_commits = Vec::new();
    let mut previous_ids = Vec::new();
    for (iteration, target) in [
        RenderTarget::Output,
        RenderTarget::ScreenCapture,
        RenderTarget::Screencast,
    ]
    .into_iter()
    .enumerate()
    {
        let state = f.niri_state();
        let (backend, niri) = (&mut state.backend, &mut state.niri);
        backend
            .with_primary_renderer(|renderer| {
                let mapped = niri.mapped_layer_surfaces.values().next().unwrap();
                let mut elements = Vec::new();
                mapped.render_normal(
                    RenderCtx {
                        renderer,
                        target,
                        xray: None,
                    },
                    None,
                    Point::from((0., 0.)),
                    XrayPos::default(),
                    &mut |element| elements.push(element),
                );
                assert_eq!(
                    elements.len(),
                    2,
                    "two independent planes from one root texture"
                );
                assert_ne!(elements[0].id(), elements[1].id());
                let root_id = Id::from_wayland_resource(mapped.surface().wl_surface());
                assert!(elements.iter().all(|element| element.id() != &root_id));
                if iteration == 0 {
                    previous_commits = elements.iter().map(Element::current_commit).collect();
                    previous_ids = elements
                        .iter()
                        .map(|element| element.id().clone())
                        .collect();
                } else {
                    for (index, element) in elements.iter().enumerate() {
                        assert_eq!(element.id(), &previous_ids[index]);
                        assert!(element
                            .damage_since(Scale::from(1.5), Some(previous_commits[index]))
                            .is_empty());
                    }
                }
                let pixels = render_helpers::render_to_vec(
                    renderer,
                    Size::from((144, 108)),
                    Scale::from(1.5),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    elements.into_iter().rev(),
                )
                .expect("render and read plane pixels");
                for y in 0..108 {
                    for x in 0..144 {
                        let alpha: u8 = if (24..48).contains(&x) && (12..36).contains(&y) {
                            255
                        } else if (72..96).contains(&x) && (24..72).contains(&y) {
                            128
                        } else {
                            0
                        };
                        let at = (y * 144 + x) * 4;
                        let actual = &pixels[at..at + 4];
                        let expected = [alpha, 0, 0, alpha];
                        assert!(
                            actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
                            "target={target:?} x={x} y={y} actual={actual:?} expected={expected:?}"
                        );
                    }
                }
            })
            .unwrap();
    }

    // An opacity-only change must damage the plane although neither its buffer
    // commit nor its destination/clip changed. The hidden plane accepts no input.
    {
        let mut runtime = runtime.lock().unwrap();
        runtime.current[1].1.opacity = 0.;
        runtime.planes[1].damage.damage_all();
    }
    let state = f.niri_state();
    let (backend, niri) = (&mut state.backend, &mut state.niri);
    backend
        .with_primary_renderer(|renderer| {
            let mapped = niri.mapped_layer_surfaces.values().next().unwrap();
            assert_eq!(
                mapped.motion_input_point(Point::from((52., 20.))),
                Some(None)
            );
            let mut elements = Vec::new();
            mapped.render_normal(
                RenderCtx {
                    renderer,
                    target: RenderTarget::Output,
                    xray: None,
                },
                None,
                Point::from((0., 0.)),
                XrayPos::default(),
                &mut |element| elements.push(element),
            );
            let changed = elements
                .iter()
                .find(|element| element.id() == &previous_ids[1]);
            // A renderer may omit a fully transparent element entirely; otherwise
            // its new commit must invalidate the former visible pixels.
            if let Some(changed) = changed {
                assert!(!changed
                    .damage_since(Scale::from(1.5), Some(previous_commits[1]))
                    .is_empty());
            }
        })
        .unwrap();
}

#[test]
fn reset_is_commit_staged_clears_atlas_and_reuses_controller_for_new_content() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.niri_state()
        .backend
        .headless()
        .add_renderer()
        .expect("surfaceless GLES renderer for reset pixels");
    let client_id = f.add_client();
    let layer = f
        .client(client_id)
        .create_layer(None, Layer::Top, "motion-reset");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((96, 72)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(client_id);

    let motion_proxy = {
        let client = f.client(client_id);
        client.state.layer_motion(&surface).motion.clone()
    };
    motion_proxy.add_plane(
        7, 0., 0., 96., 72., -60., 0., 96., 72., 0., 0., 96., 72., 1.,
    );
    motion_proxy.set_plane_to(7, 0., 0., 96., 72., 0., 0., 96., 72., 1.);
    motion_proxy.set_curve(Curve::Linear);
    motion_proxy.set_plane_opacity_mode(7, WireOpacityMode::Linear, 0);
    motion_proxy.animate(40, 100, 0);

    let red_buffer = {
        let layer = f.client(client_id).layer(&surface);
        layer
            .spbm
            .create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &layer.qh, ())
    };
    let mut last_acked_configure = None;
    {
        let layer = f.client(client_id).layer(&surface);
        ack_new_layer_configure(&layer, &mut last_acked_configure);
        layer.surface.attach(Some(&red_buffer), 0, 0);
        layer.set_size(96, 72);
        layer.commit();
    }
    f.double_roundtrip(client_id);

    let root_surface = f
        .niri()
        .mapped_layer_surfaces
        .keys()
        .find(|layer| layer.namespace() == "motion-reset")
        .unwrap()
        .wl_surface()
        .clone();
    f.niri()
        .mapped_layer_surfaces
        .values_mut()
        .find(|mapped| mapped.surface().namespace() == "motion-reset")
        .unwrap()
        .update_sizes(Size::from((96., 72.)), 1.5);
    let runtime = f
        .state
        .server
        .state
        .niri
        .layer_motion_manager_state
        .runtime(&root_surface)
        .unwrap();
    let start = runtime
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .start_time();
    let mid = start + Duration::from_millis(40);
    f.state
        .server
        .state
        .niri
        .layer_motion_manager_state
        .tick(&root_surface, mid);
    let output = f.niri_output(1);
    f.niri()
        .output_state
        .get_mut(&output)
        .unwrap()
        .frame_clock
        .presented(mid);
    f.niri().clock.set_unadjusted(mid);

    let old_pose = runtime.lock().unwrap().current[0].1;
    assert!(old_pose.destination.loc.x < -1.);
    let old_motion_id = runtime.lock().unwrap().planes[0].render_id.clone();
    let old_render = {
        let state = f.niri_state();
        let (backend, niri) = (&mut state.backend, &mut state.niri);
        backend
            .with_primary_renderer(|renderer| {
                let mapped = niri.mapped_layer_surfaces.values().next().unwrap();
                let mut elements = Vec::new();
                mapped.render_normal(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    None,
                    Point::from((0., 0.)),
                    XrayPos::default(),
                    &mut |element| elements.push(element),
                );
                let motion = elements
                    .iter()
                    .find(|element| element.id() == &old_motion_id)
                    .expect("compositor motion plane is rendered");
                (motion.current_commit(), motion.geometry(Scale::from(1.5)))
            })
            .unwrap()
    };

    // A registry roundtrip dispatches reset without applying it. The old
    // controller, sampled pose, input mapping and rendered plane remain live.
    motion_proxy.reset();
    f.double_roundtrip(client_id);
    assert_eq!(runtime.lock().unwrap().active.as_ref().unwrap().serial, 40);
    assert_eq!(runtime.lock().unwrap().planes.len(), 1);
    assert_eq!(runtime.lock().unwrap().current[0].1, old_pose);
    f.niri().clock.set_unadjusted(mid);
    let mapped = f.niri().mapped_layer_surfaces.values().next().unwrap();
    assert_eq!(mapped.motion_poses().unwrap()[0].1, old_pose);
    assert!(mapped
        .motion_input_point(Point::from((20., 20.)))
        .is_some_and(|hit| hit.is_some()));
    let unchanged_render = {
        let state = f.niri_state();
        let (backend, niri) = (&mut state.backend, &mut state.niri);
        backend
            .with_primary_renderer(|renderer| {
                let mapped = niri.mapped_layer_surfaces.values().next().unwrap();
                let mut elements = Vec::new();
                mapped.render_normal(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    None,
                    Point::from((0., 0.)),
                    XrayPos::default(),
                    &mut |element| elements.push(element),
                );
                let motion = elements
                    .iter()
                    .find(|element| element.id() == &old_motion_id)
                    .expect("uncommitted reset keeps the old atlas plane");
                (motion.current_commit(), motion.geometry(Scale::from(1.5)))
            })
            .unwrap()
    };
    assert_eq!(unchanged_render, old_render);

    // The reset and a distinct replacement buffer become visible atomically.
    let old_motion = runtime.lock().unwrap().active.clone().unwrap();
    let green_buffer = {
        let layer = f.client(client_id).layer(&surface);
        layer
            .spbm
            .create_u32_rgba_buffer(0, u32::MAX, 0, u32::MAX, &layer.qh, ())
    };
    {
        let layer = f.client(client_id).layer(&surface);
        ack_new_layer_configure(&layer, &mut last_acked_configure);
        layer.surface.attach(Some(&green_buffer), 0, 0);
        layer.set_size(96, 72);
    }
    let time_before_commit = f
        .niri()
        .output_state
        .get(&output)
        .unwrap()
        .frame_clock
        .next_presentation_time();
    let x_before_commit = old_motion.sample(time_before_commit)[0].1.destination.loc.x;
    {
        let layer = f.client(client_id).layer(&surface);
        layer.commit();
    }
    f.double_roundtrip(client_id);
    let time_after_commit = f
        .niri()
        .output_state
        .get(&output)
        .unwrap()
        .frame_clock
        .next_presentation_time();
    let x_after_commit = old_motion.sample(time_after_commit)[0].1.destination.loc.x;
    {
        let runtime_state = runtime.lock().unwrap();
        assert!(runtime_state.active.is_none());
        assert!(runtime_state.planes.is_empty());
        assert!(runtime_state.current.is_empty());
    }
    let events = &f
        .client(client_id)
        .state
        .motions
        .iter()
        .find(|controller| controller.motion == motion_proxy)
        .unwrap()
        .events;
    let final_state = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            bead_layer_motion_v1::Event::PlaneState { plane: 7, x, .. } => Some((index, *x)),
            _ => None,
        });
    let cancelled = events
        .iter()
        .position(|event| matches!(event, bead_layer_motion_v1::Event::Cancelled { serial: 40 }));
    assert!(
        final_state.is_some_and(|(index, x)| {
            let lower = x_before_commit.min(x_after_commit) - 1. / 256.;
            let upper = x_before_commit.max(x_after_commit) + 1. / 256.;
            index < cancelled.unwrap_or(usize::MAX) && (lower..=upper).contains(&x)
        }),
        "reset final plane state must precede cancelled with x between presentation samples {x_before_commit} and {x_after_commit}; got {final_state:?}"
    );
    assert!(cancelled.is_some(), "reset cancels the active serial");
    let root_id = Id::from_wayland_resource(&root_surface);
    let mut output_pixels = None;
    for target in [
        RenderTarget::Output,
        RenderTarget::ScreenCapture,
        RenderTarget::Screencast,
    ] {
        let state = f.niri_state();
        let (backend, niri) = (&mut state.backend, &mut state.niri);
        backend
            .with_primary_renderer(|renderer| {
                let mapped = niri.mapped_layer_surfaces.values().next().unwrap();
                assert!(mapped.motion_poses().is_none());
                assert!(mapped.motion_input_point(Point::from((20., 20.))).is_none());
                assert!(mapped
                    .surface_under_motion(Point::from((20., 20.)), WindowSurfaceType::TOPLEVEL,)
                    .is_none());
                assert!(mapped
                    .surface()
                    .surface_under(Point::from((20., 20.)), WindowSurfaceType::TOPLEVEL)
                    .is_some());
                let mut elements = Vec::new();
                mapped.render_normal(
                    RenderCtx {
                        renderer,
                        target,
                        xray: None,
                    },
                    None,
                    Point::from((0., 0.)),
                    XrayPos::default(),
                    &mut |element| elements.push(element),
                );
                let root = elements
                    .iter()
                    .find(|element| element.id() == &root_id)
                    .expect("reset content uses the ordinary root-surface path");
                assert!(!root.damage_since(Scale::from(1.5), None).is_empty());
                if target == RenderTarget::Output {
                    output_pixels = Some(
                        render_helpers::render_to_vec(
                            renderer,
                            Size::from((144, 108)),
                            Scale::from(1.5),
                            Transform::Normal,
                            Fourcc::Abgr8888,
                            elements.into_iter().rev(),
                        )
                        .expect("render ordinary replacement buffer"),
                    );
                }
            })
            .unwrap();
    }
    let output_pixels = output_pixels.expect("output capture pixels");
    let green_center = ((54 * 144 + 72) * 4) as usize;
    assert_eq!(
        &output_pixels[green_center..green_center + 4],
        &[0, 255, 0, 255]
    );

    // A reset may introduce fresh planes in the same commit. A stale serial is
    // ignored while the new plane still presents its own initial pose.
    motion_proxy.reset();
    motion_proxy.add_plane(
        7, 0., 0., 96., 72., -24., 0., 96., 72., 0., 0., 96., 72., 1.,
    );
    motion_proxy.set_plane_to(7, 0., 0., 96., 72., 0., 0., 96., 72., 1.);
    motion_proxy.set_curve(Curve::Linear);
    motion_proxy.set_plane_opacity_mode(7, WireOpacityMode::Linear, 0);
    motion_proxy.animate(40, 100, 0);
    {
        let layer = f.client(client_id).layer(&surface);
        ack_new_layer_configure(&layer, &mut last_acked_configure);
        layer.commit();
    }
    f.double_roundtrip(client_id);
    {
        let runtime_state = runtime.lock().unwrap();
        assert!(runtime_state.active.is_none());
        assert_eq!(runtime_state.planes.len(), 1);
        assert_eq!(runtime_state.current[0].1.destination.loc.x, -24.);
    }

    // The same controller accepts a newer retarget from the fresh pose, not
    // the old atlas's sampled reset pose.
    motion_proxy.animate(41, 100, 1);
    {
        let layer = f.client(client_id).layer(&surface);
        ack_new_layer_configure(&layer, &mut last_acked_configure);
        layer.commit();
    }
    f.double_roundtrip(client_id);
    let runtime_state = runtime.lock().unwrap();
    let active = runtime_state.active.as_ref().unwrap();
    assert_eq!(active.serial, 41);
    assert_eq!(runtime_state.planes.len(), 1);
    assert_eq!(runtime_state.planes[0].id, 7);
    assert_eq!(
        active.sample(active.start_time())[0].1.destination.loc.x,
        -24.
    );
}
