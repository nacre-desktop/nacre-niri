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
use crate::layer::reveal::{CornerMode, Rect as RevealRect, RevealDescriptor};
use crate::protocols::layer_motion::{LayerMotionRuntime, MotionPlane};
use crate::render_helpers::damage::ExtraDamage;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::{self, RenderCtx, RenderTarget};

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rectangle<f64, Logical> {
    Rectangle::new(Point::from((x, y)), Size::from((width, height)))
}

fn fixture_number(value: &serde_json::Value) -> f32 {
    value.as_f64().unwrap() as f32
}

fn fixture_fixed(value: f32) -> f32 {
    (value * 256.).round() / 256.
}

fn fixture_rect(value: &serde_json::Value) -> RevealRect {
    let number = |key| fixture_fixed(fixture_number(&value[key]));
    RevealRect::new(number("x"), number("y"), number("w"), number("h"))
}

fn reveal_from_fixture(source: &serde_json::Value) -> RevealDescriptor {
    let mut reveal = RevealDescriptor::empty();
    reveal.output_bounds = fixture_rect(&source["output_bounds"]);
    reveal.crop = fixture_rect(&source["crop"]);
    reveal.static_rects = source["static_rects"]
        .as_array()
        .unwrap()
        .iter()
        .map(fixture_rect)
        .collect();
    reveal.holes = source["holes"]
        .as_array()
        .unwrap()
        .iter()
        .map(fixture_rect)
        .collect();
    reveal.animated_from = fixture_rect(&source["animated_from"]);
    reveal.animated_to = fixture_rect(&source["animated_to"]);
    reveal.radii_from = source["radii_from"]
        .as_array()
        .unwrap()
        .iter()
        .map(fixture_number)
        .map(fixture_fixed)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    reveal.radii_to = source["radii_to"]
        .as_array()
        .unwrap()
        .iter()
        .map(fixture_number)
        .map(fixture_fixed)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    reveal.corner_mode = match source["corner_mode"].as_str().unwrap() {
        "QuadraticCut" => CornerMode::QuadraticCut,
        "Circular" => CornerMode::Circular,
        other => panic!("unknown corner mode {other}"),
    };
    reveal.square_edges = source["square_edges"].as_u64().unwrap() as u32;
    reveal.fill = source["fill"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    reveal.opacity_plane = source["opacity_plane"].as_u64().unwrap() as u32;
    reveal.visible = source["mode"] == "Visible";
    if !source["content_clip_from"].is_null() {
        reveal.content_clip = Some((
            fixture_rect(&source["content_clip_from"]),
            fixture_rect(&source["content_clip_to"]),
        ));
    }
    if !source["rim"].is_null() {
        let rim = &source["rim"];
        let colors = rim["colors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                c.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u8)
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap()
            })
            .collect::<Vec<[u8; 4]>>()
            .try_into()
            .unwrap();
        let stops = rim["stops"]
            .as_array()
            .unwrap()
            .iter()
            .map(fixture_number)
            .map(fixture_fixed)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let point = |key: &str| {
            let p = rim[key].as_array().unwrap();
            (
                fixture_fixed(fixture_number(&p[0])),
                fixture_fixed(fixture_number(&p[1])),
            )
        };
        reveal.rim = Some(crate::layer::reveal::RimStyle {
            colors,
            stops,
            start: point("start"),
            end: point("end"),
            width: fixture_fixed(fixture_number(&rim["width"])),
            opacity: fixture_fixed(fixture_number(&rim["opacity"])),
        });
    }
    if !source["shadow"].is_null() {
        let shadow = &source["shadow"];
        let color = shadow["color"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        reveal.shadow = Some(crate::layer::reveal::ShadowStyle {
            color,
            blur_radius: fixture_fixed(fixture_number(&shadow["blur"])),
            spread: fixture_fixed(fixture_number(&shadow["spread"])),
            offset: (
                fixture_fixed(fixture_number(&shadow["offset_x"])),
                fixture_fixed(fixture_number(&shadow["offset_y"])),
            ),
        });
    }
    reveal
}

fn thread_cpu_time() -> Duration {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    assert_eq!(result, 0, "read thread CPU clock");
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
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
        reveal: None,
        reveal_current: None,
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
fn reveal_fill_raster_is_under_content_and_survives_capture_rendering() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.niri_state()
        .backend
        .headless()
        .add_renderer()
        .expect("surfaceless GLES renderer for contour reveal pixels");
    let client_id = f.add_client();
    let layer = f
        .client(client_id)
        .create_layer(None, Layer::Top, "motion-reveal-render");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((32, 32)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(client_id);
    let layer = f.client(client_id).layer(&surface);
    let buffer = layer
        .spbm
        .create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &layer.qh, ());
    layer.surface.attach(Some(&buffer), 0, 0);
    layer.set_size(32, 32);
    layer.ack_last_and_commit();
    f.double_roundtrip(client_id);

    let pose = MotionPose {
        destination: rect(8., 8., 16., 16.),
        clip: rect(0., 0., 32., 32.),
        opacity: 1.,
    };
    let mut reveal = RevealDescriptor::empty();
    reveal.output_bounds = RevealRect::new(0., 0., 32., 32.);
    reveal.crop = reveal.output_bounds;
    reveal.animated_from = RevealRect::new(4., 4., 24., 24.);
    reveal.animated_to = reveal.animated_from;
    reveal.radii_from = [4.; 4];
    reveal.radii_to = [4.; 4];
    reveal.corner_mode = CornerMode::QuadraticCut;
    reveal.fill = [0, 0, 255, 255];
    let runtime = Arc::new(Mutex::new(LayerMotionRuntime {
        planes: vec![plane(0, rect(0., 0., 32., 32.), pose)],
        current: vec![(0, pose)],
        active: None,
        reveal: Some(reveal.clone()),
        reveal_current: Some(reveal.sample(1.)),
    }));
    let mapped = f.niri().mapped_layer_surfaces.values_mut().next().unwrap();
    mapped.update_sizes(Size::from((32., 32.)), 1.);
    mapped.set_motion_runtime(runtime);

    let pixels = {
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
                assert_eq!(elements.len(), 2, "one content plane plus contour texture");
                let reveal_id = elements[1].id().clone();
                let reveal_commit = elements[1].current_commit();
                let mut repeated = Vec::new();
                mapped.render_normal(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    None,
                    Point::from((0., 0.)),
                    XrayPos::default(),
                    &mut |element| repeated.push(element),
                );
                assert_eq!(repeated[1].id(), &reveal_id);
                assert!(repeated[1]
                    .damage_since(Scale::from(1.), Some(reveal_commit))
                    .is_empty());
                render_helpers::render_to_vec(
                    renderer,
                    Size::from((32, 32)),
                    Scale::from(1.),
                    Transform::Normal,
                    Fourcc::Abgr8888,
                    repeated.into_iter().rev(),
                )
                .expect("render compositor reveal plus content")
                .to_vec()
            })
            .unwrap()
    };
    let pixel = |x: usize, y: usize| &pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4];
    assert_eq!(pixel(5, 5), &[0, 0, 255, 255]);
    assert_eq!(pixel(12, 12), &[255, 0, 0, 255]);
    assert_eq!(pixel(0, 0), &[0, 0, 0, 0]);
}

#[test]
#[ignore = "requires Nacre's exported reveal-goldens directory via NACRE_REVEAL_REFERENCE_DIR"]
fn reveal_tiny_skia_raster_matches_nacre_cpu_on_transparent_atlas_pixels() {
    use std::fs::File;
    use std::io::BufReader;

    fn png_rgba(path: &std::path::Path) -> (Vec<u8>, u32, u32) {
        let decoder = png::Decoder::new(BufReader::new(File::open(path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut data = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut data).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba, "{}", path.display());
        data.truncate(info.buffer_size());
        (data, info.width, info.height)
    }
    fn number(value: &serde_json::Value) -> f32 {
        value.as_f64().unwrap() as f32
    }
    fn rect(value: &serde_json::Value) -> RevealRect {
        RevealRect::new(
            number(&value["x"]),
            number(&value["y"]),
            number(&value["w"]),
            number(&value["h"]),
        )
    }
    fn wire(value: f32) -> f32 {
        (value * 256.).round() / 256.
    }
    let root = std::path::PathBuf::from(
        std::env::var("NACRE_REVEAL_REFERENCE_DIR").expect("fixture directory is set"),
    );
    let mut comparisons = 0usize;
    let mut maximum_difference = 0u8;
    let mut worst: Option<(String, &'static str, usize, usize, Vec<u8>, Vec<u8>)> = None;
    for route in ["dashboard", "launcher", "control"] {
        for scale_text in ["1.0", "1.5"] {
            let stem = format!("{route}-s{scale_text}");
            let plan: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{stem}-plan.json"))).unwrap(),
            )
            .unwrap();
            let (atlas, atlas_width, atlas_height) =
                png_rgba(&root.join(&plan["atlas"]["filename"].as_str().unwrap()));
            let scale = number(&plan["atlas"]["scale"]);
            let logical_width = atlas_width as f32 / scale;
            let logical_height = atlas_height as f32 / scale;
            let source = &plan["reveal"];
            let fixed = |value: f32| wire(value);
            let wire_rect =
                |r: RevealRect| RevealRect::new(fixed(r.x), fixed(r.y), fixed(r.w), fixed(r.h));
            let mut reveal = RevealDescriptor::empty();
            reveal.output_bounds = wire_rect(rect(&source["output_bounds"]));
            reveal.crop = wire_rect(rect(&source["crop"]));
            reveal.static_rects = source["static_rects"]
                .as_array()
                .unwrap()
                .iter()
                .map(rect)
                .map(wire_rect)
                .collect();
            reveal.holes = source["holes"]
                .as_array()
                .unwrap()
                .iter()
                .map(rect)
                .map(wire_rect)
                .collect();
            let from = rect(&source["animated_from"]);
            let to = rect(&source["animated_to"]);
            reveal.animated_from =
                RevealRect::new(fixed(from.x), fixed(from.y), fixed(from.w), fixed(from.h));
            reveal.animated_to =
                RevealRect::new(fixed(to.x), fixed(to.y), fixed(to.w), fixed(to.h));
            reveal.radii_from = source["radii_from"]
                .as_array()
                .unwrap()
                .iter()
                .map(number)
                .map(fixed)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            reveal.radii_to = source["radii_to"]
                .as_array()
                .unwrap()
                .iter()
                .map(number)
                .map(fixed)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            reveal.corner_mode = CornerMode::QuadraticCut;
            reveal.square_edges = source["square_edges"].as_u64().unwrap() as u32;
            reveal.fill = source["fill"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap() as u8)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            reveal.opacity_plane = source["opacity_plane"].as_u64().unwrap() as u32;
            if !source["content_clip_from"].is_null() {
                let from = rect(&source["content_clip_from"]);
                let to = rect(&source["content_clip_to"]);
                reveal.content_clip = Some((wire_rect(from), wire_rect(to)));
            }
            if !source["rim"].is_null() {
                let rim = &source["rim"];
                let colors = rim["colors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|color| {
                        color
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_u64().unwrap() as u8)
                            .collect::<Vec<_>>()
                            .try_into()
                            .unwrap()
                    })
                    .collect::<Vec<[u8; 4]>>()
                    .try_into()
                    .unwrap();
                let stops = rim["stops"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(number)
                    .map(fixed)
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
                let start = &rim["start"];
                let end = &rim["end"];
                reveal.rim = Some(crate::layer::reveal::RimStyle {
                    colors,
                    stops,
                    start: (fixed(number(&start[0])), fixed(number(&start[1]))),
                    end: (fixed(number(&end[0])), fixed(number(&end[1]))),
                    width: fixed(number(&rim["width"])),
                    opacity: fixed(number(&rim["opacity"])),
                });
            }
            let plane = &plan["planes"][0];
            let pose_from = &plane["from"];
            let pose_to = &plane["to"];
            let source_rect = wire_rect(rect(&plane["source"]));
            let clip_from = wire_rect(rect(&pose_from["clip"]));
            let pose_clip_to = wire_rect(rect(&pose_to["clip"]));
            let from_rect = wire_rect(rect(&pose_from["destination"]));
            let to_rect = wire_rect(rect(&pose_to["destination"]));
            for timeline in ["0.00", "0.25", "0.50", "0.75", "1.00"] {
                let frame_json: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(root.join(format!("{stem}-t{timeline}.json"))).unwrap(),
                )
                .unwrap();
                let progress = number(&frame_json["eased_progress"]);
                let (reference, ref_width, ref_height) =
                    png_rgba(&root.join(format!("{stem}-t{timeline}.png")));
                let (raster, crop) = reveal
                    .rasterize(progress as f64, scale, (logical_width, logical_height))
                    .unwrap();
                assert_eq!((ref_width, ref_height), (atlas_width, atlas_height));
                assert_eq!(
                    (raster.width(), raster.height()),
                    (
                        (crop.w * scale).round() as u32,
                        (crop.h * scale).round() as u32
                    ),
                    "{stem} t={timeline}"
                );
                let interp = |a: f32, b: f32| a + (b - a) * progress;
                let destination = RevealRect::new(
                    interp(from_rect.x, to_rect.x),
                    interp(from_rect.y, to_rect.y),
                    interp(from_rect.w, to_rect.w),
                    interp(from_rect.h, to_rect.h),
                );
                let clip_rect = RevealRect::new(
                    interp(clip_from.x, pose_clip_to.x),
                    interp(clip_from.y, pose_clip_to.y),
                    interp(clip_from.w, pose_clip_to.w),
                    interp(clip_from.h, pose_clip_to.h),
                );
                let opacity_from = fixed(number(&pose_from["opacity"]));
                let opacity_to = fixed(number(&pose_to["opacity"]));
                let opacity = opacity_from + (opacity_to - opacity_from) * progress;
                let opacity = if plane["opacity_mode"]["kind"] == "opaque_above"
                    && opacity > number(&plane["opacity_mode"]["threshold"])
                {
                    1.
                } else {
                    opacity
                };
                let plane_width = plan["atlas"]["width"].as_u64().unwrap() as usize;
                let mut frame_count = 0usize;
                for y in 0..ref_height as usize {
                    for x in 0..ref_width as usize {
                        let sample_x = (x as f32 + 0.5) / scale;
                        let sample_y = (y as f32 + 0.5) / scale;
                        let inside_clip = sample_x >= clip_rect.x
                            && sample_y >= clip_rect.y
                            && sample_x < clip_rect.x + clip_rect.w
                            && sample_y < clip_rect.y + clip_rect.h;
                        let inside_destination = sample_x >= destination.x
                            && sample_y >= destination.y
                            && sample_x < destination.x + destination.w
                            && sample_y < destination.y + destination.h;
                        let source_alpha = if inside_clip && inside_destination && opacity > 0. {
                            let sx = source_rect.x
                                + (sample_x - destination.x) / destination.w * source_rect.w;
                            let sy = source_rect.y
                                + (sample_y - destination.y) / destination.h * source_rect.h;
                            let ax = (sx * scale).floor() as isize;
                            let ay = (sy * scale).floor() as isize;
                            (-1..=1).any(|dy| {
                                (-1..=1).any(|dx| {
                                    let px = ax + dx;
                                    let py = ay + dy;
                                    px >= 0
                                        && py >= 0
                                        && px < atlas_width as isize
                                        && py < atlas_height as isize
                                        && atlas[(py as usize * plane_width + px as usize) * 4 + 3]
                                            != 0
                                })
                            })
                        } else {
                            false
                        };
                        if source_alpha {
                            continue;
                        }
                        let physical_crop_x = crop.x * scale;
                        let physical_crop_y = crop.y * scale;
                        let tx = (x as f32 + 0.5 - physical_crop_x).floor() as isize;
                        let ty = (y as f32 + 0.5 - physical_crop_y).floor() as isize;
                        let expected = if tx >= 0
                            && ty >= 0
                            && tx < raster.width() as isize
                            && ty < raster.height() as isize
                        {
                            &raster.data()
                                [(ty as usize * raster.width() as usize + tx as usize) * 4..][..4]
                        } else {
                            &[0, 0, 0, 0]
                        };
                        let actual = &reference[(y * ref_width as usize + x) * 4..][..4];
                        for channel in 0..4 {
                            let expected_channel =
                                (expected[channel] as f32 * opacity).round() as u8;
                            let difference = expected_channel.abs_diff(actual[channel]);
                            if difference > maximum_difference {
                                maximum_difference = difference;
                                worst = Some((
                                    stem.clone(),
                                    timeline,
                                    x,
                                    y,
                                    expected
                                        .iter()
                                        .map(|v| (*v as f32 * opacity).round() as u8)
                                        .collect::<Vec<_>>(),
                                    actual.to_vec(),
                                ));
                            }
                        }
                        frame_count += 1;
                    }
                }
                comparisons += frame_count;
            }
        }
    }
    assert!(comparisons > 0);
    assert!(
        maximum_difference <= 1,
        "max diff={maximum_difference}, worst={worst:?}"
    );
}

#[test]
fn reveal_full_gles_frames_match_nacre_goldens_at_both_scales() {
    use std::fs::File;
    use std::io::BufReader;
    use std::time::Instant;

    use smithay::backend::renderer::ExportMem as _;

    fn png_rgba(path: &std::path::Path) -> (Vec<u8>, u32, u32) {
        let decoder = png::Decoder::new(BufReader::new(File::open(path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut data = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut data).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba);
        data.truncate(info.buffer_size());
        (data, info.width, info.height)
    }
    fn n(value: &serde_json::Value) -> f32 {
        value.as_f64().unwrap() as f32
    }
    fn rect(value: &serde_json::Value) -> RevealRect {
        RevealRect::new(
            n(&value["x"]),
            n(&value["y"]),
            n(&value["w"]),
            n(&value["h"]),
        )
    }
    fn fixed(value: f32) -> f32 {
        (value * 256.).round() / 256.
    }
    let root = std::env::var_os("NACRE_REVEAL_REFERENCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("src/tests/fixtures/nacre-reveal-goldens")
        });
    let mut compared = 0usize;
    let mut route_failures = Vec::new();
    for route in ["dashboard", "launcher", "control"] {
        for scale_text in ["1.0", "1.5"] {
            let stem = format!("{route}-s{scale_text}");
            let plan: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{stem}-plan.json"))).unwrap(),
            )
            .unwrap();
            let (atlas, atlas_w, atlas_h) =
                png_rgba(&root.join(plan["atlas"]["filename"].as_str().unwrap()));
            let scale = n(&plan["atlas"]["scale"]);
            let width = atlas_w as f32 / scale;
            let height = atlas_h as f32 / scale;
            let reveal = reveal_from_fixture(&plan["reveal"]);

            let plan_plane = &plan["planes"][0];
            let source_rect = fixture_rect(&plan_plane["source"]);
            let pose = |field: &str| {
                let pose = &plan_plane[field];
                let dest = fixture_rect(&pose["destination"]);
                let clip = fixture_rect(&pose["clip"]);
                MotionPose::new(
                    dest.x as f64,
                    dest.y as f64,
                    dest.w as f64,
                    dest.h as f64,
                    clip.x as f64,
                    clip.y as f64,
                    clip.w as f64,
                    clip.h as f64,
                    fixed(n(&pose["opacity"])) as f64,
                )
                .unwrap()
            };
            let from = pose("from");
            let to = pose("to");
            let opacity_mode = if plan_plane["opacity_mode"]["kind"] == "opaque_above" {
                OpacityMode::OpaqueAbove(
                    (n(&plan_plane["opacity_mode"]["threshold"]) * 1_000_000.).round() as u32,
                )
            } else {
                OpacityMode::Linear
            };
            let curve = match plan["curve"].as_str().unwrap() {
                "Linear" => crate::layer::motion::MotionCurve::Linear,
                "EaseOut" => crate::layer::motion::MotionCurve::EaseOutCubic,
                "EaseInOut" => crate::layer::motion::MotionCurve::EaseInOutCubic,
                other => panic!("unsupported fixture curve {other}"),
            };
            let duration = Duration::from_secs_f64(plan["duration_ms"].as_f64().unwrap() / 1000.);
            let group = crate::layer::motion::LayerMotionGroup::new(
                1,
                vec![(plan_plane["id"].as_u64().unwrap() as u32, from)],
                vec![(plan_plane["id"].as_u64().unwrap() as u32, to)],
                vec![(plan_plane["id"].as_u64().unwrap() as u32, opacity_mode)],
                Duration::ZERO,
                duration,
                curve,
            )
            .unwrap();
            let mut f = Fixture::new();
            f.add_output(1, (1920, 1200));
            f.niri_state()
                .backend
                .headless()
                .add_renderer()
                .expect("surfaceless GLES renderer for Nacre reveal goldens");
            let client_id = f.add_client();
            let namespace = format!("motion-golden-{stem}");
            let layer = f
                .client(client_id)
                .create_layer(None, Layer::Top, &namespace);
            let surface = layer.surface.clone();
            layer.set_configure_props(LayerConfigureProps {
                anchor: Some(Anchor::Left | Anchor::Top),
                size: Some((width as u32, height as u32)),
                ..Default::default()
            });
            layer.commit();
            f.double_roundtrip(client_id);
            let buffer = f
                .client(client_id)
                .create_rgba_shm_buffer(atlas_w, atlas_h, &atlas);
            let layer = f.client(client_id).layer(&surface);
            layer.surface.attach(Some(&buffer), 0, 0);
            layer.set_size(width as u16, height as u16);
            layer.ack_last_and_commit();
            f.double_roundtrip(client_id);
            let mapped = f
                .niri()
                .mapped_layer_surfaces
                .values_mut()
                .find(|mapped| mapped.surface().namespace() == namespace)
                .unwrap();
            mapped.update_sizes(Size::from((width as f64, height as f64)), scale as f64);
            let motion_plane = plane(
                plan_plane["id"].as_u64().unwrap() as u32,
                Rectangle::new(
                    Point::from((source_rect.x as f64, source_rect.y as f64)),
                    Size::from((source_rect.w as f64, source_rect.h as f64)),
                ),
                from,
            );
            mapped.set_motion_runtime(Arc::new(Mutex::new(LayerMotionRuntime {
                planes: vec![motion_plane],
                current: group.sample(Duration::ZERO),
                active: Some(group),
                reveal: Some(reveal.clone()),
                reveal_current: Some(reveal.sample(0.)),
            })));
            let format = Fourcc::Abgr8888;
            let mut route_max = 0u8;
            let mut route_worst = None;
            let mut element_build_wall = Vec::new();
            let mut element_build_cpu = Vec::new();
            let mut frame_submit_wall = Vec::new();
            let mut frame_submit_cpu = Vec::new();
            let mut frame_map_wall = Vec::new();
            let mut frame_map_cpu = Vec::new();
            for timeline in ["0.00", "0.25", "0.50", "0.75", "1.00"] {
                let frame_json: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(root.join(format!("{stem}-t{timeline}.json"))).unwrap(),
                )
                .unwrap();
                let time =
                    Duration::from_secs_f64(frame_json["sample_time_ms"].as_f64().unwrap() / 1000.);
                let sampled_progress = n(&frame_json["eased_progress"]) as f64;
                f.niri().clock.set_unadjusted(time);
                let output = f.niri_output(1);
                f.niri()
                    .output_state
                    .get_mut(&output)
                    .unwrap()
                    .frame_clock
                    .presented(time);
                let (
                    pixels,
                    template_mode,
                    build_wall,
                    build_cpu,
                    submit_wall,
                    submit_cpu,
                    map_wall,
                    map_cpu,
                ) = {
                    let state = f.niri_state();
                    let (backend, niri) = (&mut state.backend, &mut state.niri);
                    backend
                        .with_primary_renderer(|renderer| {
                            let mapped = niri
                                .mapped_layer_surfaces
                                .values()
                                .find(|mapped| mapped.surface().namespace() == namespace)
                                .unwrap();
                            let mut elements = Vec::new();
                            let build_cpu_start = thread_cpu_time();
                            let build_wall_start = Instant::now();
                            mapped.render_normal(
                                RenderCtx {
                                    renderer,
                                    target: RenderTarget::Output,
                                    xray: None,
                                },
                                None,
                                // Render into the layer's buffer-local capture rectangle. The
                                // layer surface's output-global origin is applied by the output
                                // compositor and is deliberately omitted from this local target.
                                Point::from((0., 0.)),
                                XrayPos::default(),
                                &mut |element| elements.push(element),
                            );
                            let build_wall = build_wall_start.elapsed();
                            let build_cpu = thread_cpu_time() - build_cpu_start;
                            let template_mode = elements.iter().any(|element| {
                                matches!(
                                    element,
                                    crate::layer::mapped::LayerSurfaceRenderElement::RevealClip(_)
                                )
                            });
                            let submit_cpu_start = thread_cpu_time();
                            let submit_wall_start = Instant::now();
                            let mapping = render_helpers::render_and_download(
                                renderer,
                                Size::from((atlas_w as i32, atlas_h as i32)),
                                Scale::from(scale as f64),
                                Transform::Normal,
                                format,
                                elements.into_iter().rev(),
                            )
                            .expect("draw actual compositor reveal frame");
                            let submit_wall = submit_wall_start.elapsed();
                            let submit_cpu = thread_cpu_time() - submit_cpu_start;
                            let map_cpu_start = thread_cpu_time();
                            let map_wall_start = Instant::now();
                            let pixels = renderer
                                .map_texture(&mapping)
                                .expect("wait for and map full GLES frame")
                                .to_vec();
                            let map_wall = map_wall_start.elapsed();
                            let map_cpu = thread_cpu_time() - map_cpu_start;
                            (
                                pixels,
                                template_mode,
                                build_wall,
                                build_cpu,
                                submit_wall,
                                submit_cpu,
                                map_wall,
                                map_cpu,
                            )
                        })
                        .unwrap()
                };
                element_build_wall.push(build_wall);
                element_build_cpu.push(build_cpu);
                frame_submit_wall.push(submit_wall);
                frame_submit_cpu.push(submit_cpu);
                frame_map_wall.push(map_wall);
                frame_map_cpu.push(map_cpu);
                let (reference, ref_width, ref_height) =
                    png_rgba(&root.join(format!("{stem}-t{timeline}.png")));
                assert_eq!((ref_width, ref_height), (atlas_w, atlas_h));
                let mut frame_max = 0u8;
                let mut frame_worst = None;
                for index in (0..pixels.len()).step_by(4) {
                    for channel in 0..4 {
                        let difference =
                            pixels[index + channel].abs_diff(reference[index + channel]);
                        if difference > frame_max {
                            frame_max = difference;
                            frame_worst = Some((
                                index / 4,
                                channel,
                                pixels[index..index + 4].to_vec(),
                                reference[index..index + 4].to_vec(),
                            ));
                        }
                        if difference > route_max {
                            route_max = difference;
                            route_worst = Some((
                                timeline,
                                index / 4,
                                channel,
                                pixels[index..index + 4].to_vec(),
                                reference[index..index + 4].to_vec(),
                            ));
                        }
                        compared += 1;
                    }
                }
                println!(
                    "reveal-full-frame {stem} t{timeline}: template_mode={template_mode} max_diff={frame_max} worst={frame_worst:?}"
                );
                if frame_max > 1 {
                    let pixel_index = frame_worst.as_ref().unwrap().0;
                    let x = pixel_index % atlas_w as usize;
                    let y = pixel_index / atlas_w as usize;
                    let atlas_offset = pixel_index * 4;
                    let atlas_pixel = &atlas[atlas_offset..atlas_offset + 4];
                    let (direct, crop) = reveal
                        .rasterize(sampled_progress, scale, (width, height))
                        .expect("build direct compositor contour diagnostic");
                    let contour_x = x as i32 - (crop.x * scale).round() as i32;
                    let contour_y = y as i32 - (crop.y * scale).round() as i32;
                    let contour_pixel = if contour_x >= 0
                        && contour_y >= 0
                        && contour_x < direct.width() as i32
                        && contour_y < direct.height() as i32
                    {
                        let offset =
                            (contour_y as usize * direct.width() as usize + contour_x as usize) * 4;
                        direct.data()[offset..offset + 4].to_vec()
                    } else {
                        Vec::new()
                    };
                    println!(
                        "reveal-full-frame {stem} t{timeline} diagnostic pixel=({x},{y}) template_mode={template_mode} atlas={atlas_pixel:?} direct_contour={contour_pixel:?} compositor={:?} reference={:?}",
                        &pixels[pixel_index * 4..pixel_index * 4 + 4],
                        &reference[pixel_index * 4..pixel_index * 4 + 4],
                    );
                }
            }
            println!("reveal-full-frame {stem}: max_diff={route_max} worst={route_worst:?}");
            if route_max > 1 {
                route_failures.push(format!(
                    "{stem}: max diff={route_max}, worst={route_worst:?}"
                ));
            }
            let report = |phase: &str, wall: &[Duration], cpu: &[Duration]| {
                let mean =
                    wall.iter().sum::<Duration>().as_secs_f64() * 1_000_000. / wall.len() as f64;
                let cpu_mean =
                    cpu.iter().sum::<Duration>().as_secs_f64() * 1_000_000. / cpu.len() as f64;
                println!(
                    "reveal-full-frame {stem} {phase}: n={} mean_wall_us={mean:.2} mean_thread_cpu_us={cpu_mean:.2}",
                    wall.len()
                );
            };
            report(
                "element-build-raster-upload",
                &element_build_wall,
                &element_build_cpu,
            );
            report("draw-submit-copy", &frame_submit_wall, &frame_submit_cpu);
            report("fence-wait-map", &frame_map_wall, &frame_map_cpu);
        }
    }
    assert!(compared > 0);
    assert!(
        route_failures.is_empty(),
        "compositor reveal golden mismatches:\n{}",
        route_failures.join("\n")
    );
}

#[test]
#[ignore = "headless GLES compositor cost benchmark; run explicitly on owner desktop"]
fn reveal_headless_gles_cost_benchmark() {
    use std::time::Instant;

    const FRAMES: usize = 64;
    let root = std::path::PathBuf::from(
        std::env::var("NACRE_REVEAL_REFERENCE_DIR").expect("fixture directory is set"),
    );
    struct Case {
        label: String,
        scale: f64,
        width: f32,
        height: f32,
        atlas: Vec<u8>,
        atlas_width: u32,
        atlas_height: u32,
        source: RevealRect,
        reveal: RevealDescriptor,
        group: crate::layer::motion::LayerMotionGroup,
        duration: Duration,
    }
    let mut cases = Vec::new();
    for route in ["dashboard", "launcher", "control"] {
        for scale_text in ["1.0", "1.5"] {
            let stem = format!("{route}-s{scale_text}");
            let plan: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{stem}-plan.json"))).unwrap(),
            )
            .unwrap();
            let scale = fixture_number(&plan["atlas"]["scale"]) as f64;
            let atlas_path = root.join(plan["atlas"]["filename"].as_str().unwrap());
            let decoder = png::Decoder::new(std::io::BufReader::new(
                std::fs::File::open(atlas_path).unwrap(),
            ));
            let mut reader = decoder.read_info().unwrap();
            let mut atlas = vec![0; reader.output_buffer_size().unwrap()];
            let atlas_info = reader.next_frame(&mut atlas).unwrap();
            assert_eq!(atlas_info.color_type, png::ColorType::Rgba);
            atlas.truncate(atlas_info.buffer_size());
            let atlas_width = atlas_info.width;
            let atlas_height = atlas_info.height;
            let width = atlas_width as f32 / scale as f32;
            let height = atlas_height as f32 / scale as f32;
            let plane = &plan["planes"][0];
            let source = fixture_rect(&plane["source"]);
            let pose = |field: &str| {
                let value = &plane[field];
                let destination = fixture_rect(&value["destination"]);
                let clip = fixture_rect(&value["clip"]);
                MotionPose::new(
                    destination.x as f64,
                    destination.y as f64,
                    destination.w as f64,
                    destination.h as f64,
                    clip.x as f64,
                    clip.y as f64,
                    clip.w as f64,
                    clip.h as f64,
                    fixture_fixed(fixture_number(&value["opacity"])) as f64,
                )
                .unwrap()
            };
            let opacity = if plane["opacity_mode"]["kind"] == "opaque_above" {
                OpacityMode::OpaqueAbove(
                    (fixture_number(&plane["opacity_mode"]["threshold"]) * 1_000_000.).round()
                        as u32,
                )
            } else {
                OpacityMode::Linear
            };
            let curve = match plan["curve"].as_str().unwrap() {
                "Linear" => crate::layer::motion::MotionCurve::Linear,
                "EaseOut" => crate::layer::motion::MotionCurve::EaseOutCubic,
                "EaseInOut" => crate::layer::motion::MotionCurve::EaseInOutCubic,
                other => panic!("unsupported fixture curve {other}"),
            };
            let duration = Duration::from_secs_f64(plan["duration_ms"].as_f64().unwrap() / 1000.);
            let plane_id = plane["id"].as_u64().unwrap() as u32;
            let group = crate::layer::motion::LayerMotionGroup::new(
                1,
                vec![(plane_id, pose("from"))],
                vec![(plane_id, pose("to"))],
                vec![(plane_id, opacity)],
                Duration::ZERO,
                duration,
                curve,
            )
            .unwrap();
            cases.push(Case {
                label: stem,
                scale,
                width,
                height,
                atlas,
                atlas_width,
                atlas_height,
                source,
                reveal: reveal_from_fixture(&plan["reveal"]),
                group,
                duration,
            });
        }
    }

    fn summarize(label: &str, wall: &mut [f64], cpu: &[f64]) {
        wall.sort_by(f64::total_cmp);
        let mean = wall.iter().sum::<f64>() / wall.len() as f64;
        let cpu_mean = cpu.iter().sum::<f64>() / cpu.len() as f64;
        println!(
            "reveal-gles-bench {label}: n={} mean_wall_us={mean:.2} median_wall_us={:.2} p95_wall_us={:.2} mean_thread_cpu_us={cpu_mean:.2}",
            wall.len(),
            wall[wall.len() / 2],
            wall[wall.len() * 95 / 100],
        );
    }

    let mut f = Fixture::new();
    f.add_output(1, (1920, 1200));
    f.niri_state()
        .backend
        .headless()
        .add_renderer()
        .expect("surfaceless GLES renderer for reveal cost benchmark");
    for case in &mut cases {
        let client_id = f.add_client();
        let namespace = format!("motion-bench-{}", case.label);
        let layer = f
            .client(client_id)
            .create_layer(None, Layer::Top, &namespace);
        let surface = layer.surface.clone();
        layer.set_configure_props(LayerConfigureProps {
            anchor: Some(Anchor::Left | Anchor::Top),
            size: Some((case.width as u32, case.height as u32)),
            ..Default::default()
        });
        layer.commit();
        f.double_roundtrip(client_id);
        let buffer = f.client(client_id).create_rgba_shm_buffer(
            case.atlas_width,
            case.atlas_height,
            &case.atlas,
        );
        let layer = f.client(client_id).layer(&surface);
        layer.surface.attach(Some(&buffer), 0, 0);
        layer.set_size(case.width as u16, case.height as u16);
        layer.ack_last_and_commit();
        f.double_roundtrip(client_id);
        case.atlas.clear();
        let initial = case.group.sample(Duration::ZERO);
        let (plane_id, initial_pose) = initial[0];
        let mapped = f
            .niri()
            .mapped_layer_surfaces
            .values_mut()
            .find(|mapped| mapped.surface().namespace() == namespace)
            .unwrap();
        mapped.update_sizes(
            Size::from((case.width as f64, case.height as f64)),
            case.scale,
        );
        mapped.set_motion_runtime(Arc::new(Mutex::new(LayerMotionRuntime {
            planes: vec![plane(
                plane_id,
                Rectangle::new(
                    Point::from((case.source.x as f64, case.source.y as f64)),
                    Size::from((case.source.w as f64, case.source.h as f64)),
                ),
                initial_pose,
            )],
            current: initial,
            active: Some(case.group.clone()),
            reveal: Some(case.reveal.clone()),
            reveal_current: Some(case.reveal.sample(0.)),
        })));
    }
    let output = f.niri_output(1);
    let state = f.niri_state();
    let (backend, niri) = (&mut state.backend, &mut state.niri);
    backend
        .with_primary_renderer(|renderer| {
            println!(
                "reveal-gles-bench backend=surfaceless GLES; LP_NUM_THREADS={}; 8 warmup and 64 sampled frames per route/scale; record external eglinfo -B; one output target allocation per frame; no readback",
                std::env::var("LP_NUM_THREADS").unwrap_or_else(|_| "<unset>".into())
            );
            for case in cases {
                let namespace = format!("motion-bench-{}", case.label);
                let mut element_wall = Vec::with_capacity(FRAMES);
                let mut element_cpu = Vec::with_capacity(FRAMES);
                let mut draw_wall = Vec::with_capacity(FRAMES);
                let mut draw_cpu = Vec::with_capacity(FRAMES);
                let mut fence_wall = Vec::with_capacity(FRAMES);
                let mut fence_cpu = Vec::with_capacity(FRAMES);
                let mut first_template_frame = None;
                for frame in 0..(FRAMES + 8) {
                    let raw_progress = frame as f64 / (FRAMES + 8 - 1) as f64;
                    let elapsed = case.duration.mul_f64(raw_progress);
                    let cpu_start = thread_cpu_time();
                    let wall_start = Instant::now();
                    niri.clock.set_unadjusted(elapsed);
                    niri.output_state
                        .get_mut(&output)
                        .unwrap()
                        .frame_clock
                        .presented(elapsed);
                    let mapped = niri
                        .mapped_layer_surfaces
                        .values()
                        .find(|mapped| mapped.surface().namespace() == namespace)
                        .expect("benchmark layer remains mapped");
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
                    let element_elapsed = wall_start.elapsed();
                    let element_cpu_elapsed = thread_cpu_time() - cpu_start;
                    let template_frame = elements.iter().any(|element| {
                        matches!(
                            element,
                            crate::layer::mapped::LayerSurfaceRenderElement::RevealClip(_)
                        )
                    });

                    let cpu_start = thread_cpu_time();
                    let wall_start = Instant::now();
                    let (texture, sync) = render_helpers::render_to_texture(
                        renderer,
                        Size::from((case.atlas_width as i32, case.atlas_height as i32)),
                        Scale::from(case.scale),
                        Transform::Normal,
                        Fourcc::Abgr8888,
                        elements.into_iter().rev(),
                    )
                    .expect("submit the mapped reveal elements to a GLES target");
                    let draw_elapsed = wall_start.elapsed();
                    let draw_cpu_elapsed = thread_cpu_time() - cpu_start;

                    let cpu_start = thread_cpu_time();
                    let wall_start = Instant::now();
                    sync.wait().expect("wait for the submitted GLES frame fence");
                    let fence_elapsed = wall_start.elapsed();
                    let fence_cpu_elapsed = thread_cpu_time() - cpu_start;
                    drop(texture);
                    if template_frame && first_template_frame.is_none() {
                        first_template_frame = Some((
                            element_elapsed.as_secs_f64() * 1_000_000.,
                            element_cpu_elapsed.as_secs_f64() * 1_000_000.,
                            draw_elapsed.as_secs_f64() * 1_000_000.,
                            draw_cpu_elapsed.as_secs_f64() * 1_000_000.,
                            fence_elapsed.as_secs_f64() * 1_000_000.,
                            fence_cpu_elapsed.as_secs_f64() * 1_000_000.,
                        ));
                    }

                    // Eight startup frames warm shader compilation and texture pools.
                    if frame >= 8 {
                        element_wall.push(element_elapsed.as_secs_f64() * 1_000_000.);
                        element_cpu.push(element_cpu_elapsed.as_secs_f64() * 1_000_000.);
                        draw_wall.push(draw_elapsed.as_secs_f64() * 1_000_000.);
                        draw_cpu.push(draw_cpu_elapsed.as_secs_f64() * 1_000_000.);
                        fence_wall.push(fence_elapsed.as_secs_f64() * 1_000_000.);
                        fence_cpu.push(fence_cpu_elapsed.as_secs_f64() * 1_000_000.);
                    }
                }
                println!(
                    "reveal-gles-bench case={} scale={} content={}x{} logical edge_cache_limit_bytes={}",
                    case.label,
                    case.scale,
                    case.width,
                    case.height,
                    crate::layer::mapped::REVEAL_EDGE_CACHE_LIMIT,
                );
                if let Some((element_wall, element_cpu, draw_wall, draw_cpu, fence_wall, fence_cpu)) =
                    first_template_frame
                {
                    println!("reveal-gles-bench first-template-frame: element-build-us={element_wall:.2} element-cpu-us={element_cpu:.2} draw-submit-us={draw_wall:.2} draw-cpu-us={draw_cpu:.2} fence-wait-us={fence_wall:.2} fence-cpu-us={fence_cpu:.2}");
                } else {
                    println!("reveal-gles-bench first-template-frame=unavailable (edge template was rejected or route did not enter the cache path)");
                }
                summarize("element-build-raster-upload", &mut element_wall, &element_cpu);
                summarize("draw-submit", &mut draw_wall, &draw_cpu);
                summarize("fence-wait", &mut fence_wall, &fence_cpu);
            }
        })
        .unwrap();
}

#[test]
fn circular_reveal_shadow_gles_matches_cpu_for_offset_and_spread() {
    fn erf(x: f32) -> f32 {
        let sign = x.signum();
        let absolute = x.abs();
        let mut approximation =
            1. + (0.278393 + (0.230389 + 0.078108 * absolute * absolute) * absolute) * absolute;
        approximation *= approximation;
        sign - sign / (approximation * approximation)
    }
    fn gaussian(x: f32, sigma: f32) -> f32 {
        (-x * x / (2. * sigma * sigma)).exp() / ((2. * std::f32::consts::PI).sqrt() * sigma)
    }
    fn box_shadow_x(x: f32, y: f32, sigma: f32, radii: [f32; 4], half: (f32, f32)) -> f32 {
        let (left_radius, right_radius) = if y < 0. {
            (radii[0], radii[1])
        } else {
            (radii[3], radii[2])
        };
        let delta_left = (half.1 - left_radius - y.abs()).min(0.);
        let delta_right = (half.1 - right_radius - y.abs()).min(0.);
        let left = -half.0 + left_radius
            - (left_radius * left_radius - delta_left * delta_left)
                .max(0.)
                .sqrt();
        let right = half.0 - right_radius
            + (right_radius * right_radius - delta_right * delta_right)
                .max(0.)
                .sqrt();
        let integral_x = 0.5 + 0.5 * erf((x - right) * (0.5f32).sqrt() / sigma);
        let integral_y = 0.5 + 0.5 * erf((x - left) * (0.5f32).sqrt() / sigma);
        integral_y - integral_x
    }
    fn box_shadow(point: (f32, f32), size: (f32, f32), sigma: f32, radii: [f32; 4]) -> f32 {
        let centered = (point.0 - size.0 * 0.5, point.1 - size.1 * 0.5);
        let half = (size.0 * 0.5, size.1 * 0.5);
        let low = centered.1 - half.1;
        let high = centered.1 + half.1;
        let start = (-3. * sigma).clamp(low, high);
        let end = (3. * sigma).clamp(low, high);
        let step = (end - start) / 4.;
        let mut y = start + step * 0.5;
        let mut value = 0.;
        for _ in 0..4 {
            value += box_shadow_x(centered.0, centered.1 - y, sigma, radii, half)
                * gaussian(y, sigma)
                * step;
            y += step;
        }
        value
    }
    fn rounded_alpha(point: (f32, f32), size: (f32, f32), radii: [f32; 4], scale: f32) -> f32 {
        let (radius, center) = if point.0 < radii[0] && point.1 < radii[0] {
            (radii[0], (radii[0], radii[0]))
        } else if size.0 - radii[1] < point.0 && point.1 < radii[1] {
            (radii[1], (size.0 - radii[1], radii[1]))
        } else if size.0 - radii[2] < point.0 && size.1 - radii[2] < point.1 {
            (radii[2], (size.0 - radii[2], size.1 - radii[2]))
        } else if point.0 < radii[3] && size.1 - radii[3] < point.1 {
            (radii[3], (radii[3], size.1 - radii[3]))
        } else {
            return 1.;
        };
        let distance = ((point.0 - center.0).powi(2) + (point.1 - center.1).powi(2)).sqrt();
        let t = ((distance - radius) * scale + 0.5).clamp(0., 1.);
        1. - t * t * (3. - 2. * t)
    }

    let mut reveal = RevealDescriptor::empty();
    reveal.output_bounds = RevealRect::new(0., 0., 160., 120.);
    reveal.crop = RevealRect::new(20., 10., 120., 100.);
    reveal.animated_from = RevealRect::new(40., 30., 70., 50.);
    reveal.animated_to = reveal.animated_from;
    reveal.radii_from = [3., 6., 9., 12.];
    reveal.radii_to = reveal.radii_from;
    reveal.fill = [0, 0, 0, 0];
    reveal.corner_mode = CornerMode::Circular;
    reveal.shadow = Some(crate::layer::reveal::ShadowStyle {
        color: [255; 4],
        offset: (7., 3.),
        blur_radius: 4.,
        spread: 4.,
    });

    let mut f = Fixture::new();
    f.add_output(1, (320, 240));
    f.niri_state()
        .backend
        .headless()
        .add_renderer()
        .expect("surfaceless GLES renderer for reveal shadow pixels");
    let client_id = f.add_client();
    let layer = f
        .client(client_id)
        .create_layer(None, Layer::Top, "motion-reveal-shadow");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((160, 120)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(client_id);
    let transparent = vec![0; 160 * 120 * 4];
    let buffer = f
        .client(client_id)
        .create_rgba_shm_buffer(160, 120, &transparent);
    let layer = f.client(client_id).layer(&surface);
    layer.surface.attach(Some(&buffer), 0, 0);
    layer.set_size(160, 120);
    layer.ack_last_and_commit();
    f.double_roundtrip(client_id);
    let pose = MotionPose::new(0., 0., 160., 120., 0., 0., 160., 120., 1.).unwrap();
    {
        let mapped = f
            .niri()
            .mapped_layer_surfaces
            .values_mut()
            .find(|mapped| mapped.surface().namespace() == "motion-reveal-shadow")
            .unwrap();
        mapped.set_motion_runtime(Arc::new(Mutex::new(LayerMotionRuntime {
            planes: vec![plane(0, rect(0., 0., 160., 120.), pose)],
            current: vec![(0, pose)],
            active: None,
            reveal: Some(reveal.clone()),
            reveal_current: Some(reveal.sample(1.)),
        })));
    }

    for scale in [1., 1.5] {
        f.niri()
            .mapped_layer_surfaces
            .values_mut()
            .find(|mapped| mapped.surface().namespace() == "motion-reveal-shadow")
            .unwrap()
            .update_sizes(Size::from((160., 120.)), scale);
        let actual = {
            let state = f.niri_state();
            let (backend, niri) = (&mut state.backend, &mut state.niri);
            backend
                .with_primary_renderer(|renderer| {
                    let mapped = niri
                        .mapped_layer_surfaces
                        .values()
                        .find(|mapped| mapped.surface().namespace() == "motion-reveal-shadow")
                        .unwrap();
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
                    render_helpers::render_to_vec(
                        renderer,
                        Size::from(((320. * scale) as i32, (240. * scale) as i32)),
                        Scale::from(scale),
                        Transform::Normal,
                        Fourcc::Abgr8888,
                        elements.into_iter().rev(),
                    )
                    .expect("render circular reveal shadow through compositor GLES path")
                })
                .unwrap()
        };
        let output_width = (320. * scale) as usize;
        let output_height = (240. * scale) as usize;
        let mut expected = vec![0; output_width * output_height * 4];
        let sigma = reveal.shadow.as_ref().unwrap().blur_radius;
        let scale_f32 = scale as f32;
        let ceil = |value: f32| (value * scale_f32).ceil() / scale_f32;
        let spread = ceil(reveal.shadow.as_ref().unwrap().spread.abs())
            .copysign(reveal.shadow.as_ref().unwrap().spread);
        let offset = (
            ceil(reveal.shadow.as_ref().unwrap().offset.0),
            ceil(reveal.shadow.as_ref().unwrap().offset.1),
        );
        let extent = (sigma * 3. * scale_f32).ceil() / scale_f32;
        let body = reveal.sample(1.).body;
        let body_radii = reveal.sample(1.).radii;
        let shadow_size = (body.w + 2. * spread, body.h + 2. * spread);
        let shadow_radii = body_radii.map(|radius| radius + spread);
        let left = body.x - extent - spread + offset.0;
        let top = body.y - extent - spread + offset.1;
        let physical_left = (left * scale_f32).round();
        let physical_top = (top * scale_f32).round();
        let physical_width = (shadow_size.0 + 2. * extent) * scale_f32;
        let physical_height = (shadow_size.1 + 2. * extent) * scale_f32;
        let window_loc = (extent + spread - offset.0, extent + spread - offset.1);
        for y in 0..output_height {
            for x in 0..output_width {
                let local_physical = (
                    x as f32 + 0.5 - physical_left,
                    y as f32 + 0.5 - physical_top,
                );
                if local_physical.0 < 0.
                    || local_physical.1 < 0.
                    || local_physical.0 >= physical_width
                    || local_physical.1 >= physical_height
                {
                    continue;
                }
                let area_point = (
                    local_physical.0 / physical_width * (shadow_size.0 + 2. * extent),
                    local_physical.1 / physical_height * (shadow_size.1 + 2. * extent),
                );
                let shadow_point = (area_point.0 - extent, area_point.1 - extent);
                let window_point = (area_point.0 - window_loc.0, area_point.1 - window_loc.1);
                let cutout = if window_point.0 >= 0.
                    && window_point.0 <= body.w
                    && window_point.1 >= 0.
                    && window_point.1 <= body.h
                {
                    1. - rounded_alpha(window_point, (body.w, body.h), body_radii, scale_f32)
                } else {
                    1.
                };
                let mask = box_shadow(shadow_point, shadow_size, sigma, shadow_radii) * cutout;
                let channel = (mask.clamp(0., 1.) * 255.).round() as u8;
                let offset = (y * output_width + x) * 4;
                expected[offset..offset + 4].fill(channel);
            }
        }
        let mut maximum_difference = 0u8;
        let mut worst = None;
        for index in (0..actual.len()).step_by(4) {
            for channel in 0..4 {
                let difference = actual[index + channel].abs_diff(expected[index + channel]);
                if difference > maximum_difference {
                    maximum_difference = difference;
                    worst = Some((
                        index / 4,
                        channel,
                        actual[index..index + 4].to_vec(),
                        expected[index..index + 4].to_vec(),
                    ));
                }
            }
        }
        assert!(
            maximum_difference <= 1,
            "scale={scale}: max diff={maximum_difference}, worst={worst:?}"
        );
    }
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
