//! Exercise the actual atlas render wrappers with a surfaceless GPU renderer.

use std::sync::{Arc, Mutex};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};

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
    let layer = f.client(client_id).create_layer(None, Layer::Top, "motion-render");
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
    let buffer = layer.spbm.create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &layer.qh, ());
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
    assert_eq!(mapped.motion_input_point(Point::from((52., 20.))), Some(Some(Point::from((8., 2.)))));
    assert_eq!(mapped.motion_input_point(Point::from((12., 6.))), Some(None));

    let mut previous_commits = Vec::new();
    let mut previous_ids = Vec::new();
    for (iteration, target) in [RenderTarget::Output, RenderTarget::ScreenCapture, RenderTarget::Screencast]
        .into_iter().enumerate()
    {
        let state = f.niri_state();
        let (backend, niri) = (&mut state.backend, &mut state.niri);
        backend.with_primary_renderer(|renderer| {
            let mapped = niri.mapped_layer_surfaces.values().next().unwrap();
            let mut elements = Vec::new();
            mapped.render_normal(
                RenderCtx { renderer, target, xray: None },
                None,
                Point::from((0., 0.)),
                XrayPos::default(),
                &mut |element| elements.push(element),
            );
            assert_eq!(elements.len(), 2, "two independent planes from one root texture");
            assert_ne!(elements[0].id(), elements[1].id());
            let root_id = Id::from_wayland_resource(mapped.surface().wl_surface());
            assert!(elements.iter().all(|element| element.id() != &root_id));
            if iteration == 0 {
                previous_commits = elements.iter().map(Element::current_commit).collect();
                previous_ids = elements.iter().map(|element| element.id().clone()).collect();
            } else {
                for (index, element) in elements.iter().enumerate() {
                    assert_eq!(element.id(), &previous_ids[index]);
                    assert!(element.damage_since(Scale::from(1.5), Some(previous_commits[index])).is_empty());
                }
            }
            let pixels = render_helpers::render_to_vec(
                renderer,
                Size::from((144, 108)),
                Scale::from(1.5),
                Transform::Normal,
                Fourcc::Abgr8888,
                elements.into_iter().rev(),
            ).expect("render and read plane pixels");
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
                    assert!(actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
                        "target={target:?} x={x} y={y} actual={actual:?} expected={expected:?}");
                }
            }
        }).unwrap();
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
    backend.with_primary_renderer(|renderer| {
        let mapped = niri.mapped_layer_surfaces.values().next().unwrap();
        assert_eq!(mapped.motion_input_point(Point::from((52., 20.))), Some(None));
        let mut elements = Vec::new();
        mapped.render_normal(
            RenderCtx { renderer, target: RenderTarget::Output, xray: None },
            None, Point::from((0., 0.)), XrayPos::default(),
            &mut |element| elements.push(element),
        );
        let changed = elements.iter().find(|element| element.id() == &previous_ids[1]);
        // A renderer may omit a fully transparent element entirely; otherwise
        // its new commit must invalidate the former visible pixels.
        if let Some(changed) = changed {
            assert!(!changed.damage_since(Scale::from(1.5), Some(previous_commits[1])).is_empty());
        }
    }).unwrap();
}
