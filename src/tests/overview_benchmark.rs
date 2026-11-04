use std::time::{Duration, Instant};

use niri_config::Config;
use niri_config::animations::{Curve, EasingParams, Kind};
use smithay::backend::allocator::Fourcc;
use smithay::utils::{Scale, Transform};

use super::Fixture;
use crate::niri::{Niri, OutputRenderElements};
use crate::render_helpers::{render_to_vec as render_pixels, RenderCtx, RenderTarget};
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

fn fixture() -> Fixture {
    let mut config = Config::parse_mem(
        "output \"headless-1\" { scale 1.5; }\noutput \"headless-2\" { scale 1.0; }",
    )
    .unwrap();
    config.layout.collections.enabled = true;
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

fn create_window(fixture: &mut Fixture, client: super::client::ClientId, w: u16, h: u16) {
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
}

fn output_elements(
    fixture: &mut Fixture,
    output_idx: u8,
) -> Vec<OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>> {
    let output = fixture.niri_output(output_idx);
    fixture.niri().update_render_elements(Some(&output));
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
                },
                &output,
                false,
            )
        })
        .expect("headless EGL renderer")
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

fn sample_frame(fixture: &mut Fixture, output_idx: u8) -> (f64, f64) {
    let assembly_start = Instant::now();
    let elements = output_elements(fixture, output_idx);
    let assembly_ms = assembly_start.elapsed().as_secs_f64() * 1000.;

    let output = fixture.niri_output(output_idx);
    let size = output.current_mode().unwrap().size;
    let scale = Scale::from(output.current_scale().fractional_scale());
    let draw_start = Instant::now();
    let state = &mut fixture.state.server.state;
    state
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
        .expect("headless EGL renderer");
    (assembly_ms, draw_start.elapsed().as_secs_f64() * 1000.)
}

fn populated_fixture() -> (Fixture, super::client::ClientId, ZwlrVirtualPointerV1) {
    let mut fixture = fixture();
    let client = fixture.add_client();
    fixture.double_roundtrip(client);
    for idx in 0..8 {
        create_window(&mut fixture, client, 900, 650);
        if idx < 7 {
            fixture.niri().layout.switch_workspace_down();
        }
    }
    for _ in 0..4 {
        fixture.niri().layout.switch_workspace_up();
    }
    fixture.niri_complete_animations();
    set_time(fixture.niri(), Duration::ZERO);

    let pointer = fixture.client(client).create_virtual_pointer("headless-1");
    pointer.motion_absolute(1, 1440, 900, 2880, 1800);
    pointer.frame();
    fixture.double_roundtrip(client);
    (fixture, client, pointer)
}

fn measure(fixture: &mut Fixture, output_idx: u8, label: &str) {
    for _ in 0..10 {
        let _ = sample_frame(fixture, output_idx);
    }
    let mut assembly = Vec::with_capacity(30);
    let mut draw = Vec::with_capacity(30);
    for _ in 0..30 {
        let (assembly_ms, draw_ms) = sample_frame(fixture, output_idx);
        assembly.push(assembly_ms);
        draw.push(draw_ms);
    }
    assembly.sort_by(f64::total_cmp);
    draw.sort_by(f64::total_cmp);
    eprintln!(
        "overview output {output_idx} {label}: assembly median={:.3} p95={:.3} ms; EGL draw+readback median={:.3} p95={:.3} ms",
        assembly[15], assembly[28], draw[15], draw[28]
    );
}

fn measure_opening_sweep(fixture: &mut Fixture) {
    let mut assembly = [Vec::with_capacity(60), Vec::with_capacity(60)];
    let mut draw = [Vec::with_capacity(60), Vec::with_capacity(60)];
    for frame in 1..=60 {
        set_time(fixture.niri(), Duration::from_millis(frame * 1000 / 60));
        fixture.niri().advance_animations();
        for output_idx in [1, 2] {
            let (assembly_ms, draw_ms) = sample_frame(fixture, output_idx);
            assembly[usize::from(output_idx - 1)].push(assembly_ms);
            draw[usize::from(output_idx - 1)].push(draw_ms);
        }
    }
    for output_idx in [1u8, 2] {
        let idx = usize::from(output_idx - 1);
        assembly[idx].sort_by(f64::total_cmp);
        draw[idx].sort_by(f64::total_cmp);
        eprintln!(
            "overview output {output_idx} opening sweep (60 changing frames): assembly median={:.3} p95={:.3} ms; EGL draw+readback median={:.3} p95={:.3} ms",
            assembly[idx][30], assembly[idx][57], draw[idx][30], draw[idx][57]
        );
    }
}

fn measure_scroll(
    fixture: &mut Fixture,
    client: super::client::ClientId,
    pointer: &ZwlrVirtualPointerV1,
    start_ms: u64,
    frame_period: Duration,
    label: &str,
) -> (f64, f64, f64) {
    use wayland_client::protocol::wl_pointer;

    let before_workspace = fixture
        .niri()
        .layout
        .active_workspace()
        .unwrap()
        .id()
        .clone();
    let event_start = Instant::now();
    pointer.axis_discrete(
        start_ms as u32 + 1,
        wl_pointer::Axis::VerticalScroll,
        15.0,
        1,
    );
    // The virtual-pointer frame accumulator drops a source-only request if no
    // timestamped axis has created the frame yet, so set the source afterward.
    pointer.axis_source(wl_pointer::AxisSource::Wheel);
    pointer.frame();
    fixture.client(client).connection.flush().unwrap();
    fixture.roundtrip(client);
    let event_ms = event_start.elapsed().as_secs_f64() * 1000.;
    let after_workspace = fixture
        .niri()
        .layout
        .active_workspace()
        .unwrap()
        .id()
        .clone();
    assert_ne!(
        before_workspace, after_workspace,
        "wheel event must switch to a different workspace"
    );

    let mut frames = Vec::with_capacity(61);
    let frames_per_second = (Duration::from_secs(1).as_nanos() / frame_period.as_nanos()) as u64;
    set_time(fixture.niri(), Duration::from_millis(start_ms + 1));
    fixture.niri().advance_animations();
    frames.push(sample_frame(fixture, 1));
    for frame in 1..=60 {
        set_time(
            fixture.niri(),
            Duration::from_millis(start_ms + 1)
                + Duration::from_nanos(frame * 1_000_000_000 / frames_per_second),
        );
        fixture.niri().advance_animations();
        let (assembly_ms, draw_ms) = sample_frame(fixture, 1);
        frames.push((assembly_ms, draw_ms));
    }
    let worst = frames
        .iter()
        .map(|(assembly, draw)| assembly + draw)
        .fold(0.0, f64::max);
    let worst_assembly = frames
        .iter()
        .map(|(assembly, _)| *assembly)
        .fold(0.0, f64::max);
    let worst_draw = frames.iter().map(|(_, draw)| *draw).fold(0.0, f64::max);
    eprintln!(
        "overview {label} wheel scroll: event dispatch+roundtrip={event_ms:.3} ms; worst changing frame total={worst:.3} ms (assembly={:.3} ms, EGL draw+readback={:.3} ms)",
        worst_assembly, worst_draw,
    );
    (worst, worst_assembly, worst_draw)
}

#[test]
#[ignore = "headless first-versus-second overview wheel benchmark; run explicitly in CI"]
fn egl_benchmark_overview_first_vs_second_wheel_scroll() {
    let (mut during_open, open_client, open_pointer) = populated_fixture();
    during_open.niri().layout.open_overview();
    for frame in 1..=4 {
        set_time(
            during_open.niri(),
            Duration::from_nanos(frame * 1_000_000_000 / 120),
        );
        during_open.niri().advance_animations();
        let _ = sample_frame(&mut during_open, 1);
    }
    let _ = measure_scroll(
        &mut during_open,
        open_client,
        &open_pointer,
        33,
        Duration::from_nanos(1_000_000_000 / 120),
        "first during 120 Hz opening (diagnostic only)",
    );

    let (mut fixture, client, pointer) = populated_fixture();
    fixture.niri().layout.open_overview();

    // Finish the opening animation before measuring the first scroll. The loop
    // renders all opening frames, matching the user's sequence of entering overview
    // and then using the wheel, without consuming a scroll event as warm-up.
    for frame in 1..=120 {
        set_time(
            fixture.niri(),
            Duration::from_nanos(frame * 1_000_000_000 / 120),
        );
        fixture.niri().advance_animations();
        let _ = sample_frame(&mut fixture, 1);
    }

    let frame_period = Duration::from_nanos(1_000_000_000 / 120);
    let (first, first_assembly, first_draw) =
        measure_scroll(&mut fixture, client, &pointer, 1000, frame_period, "first");
    fixture.niri_complete_animations();
    let (second, second_assembly, second_draw) =
        measure_scroll(&mut fixture, client, &pointer, 2000, frame_period, "second");
    let margin = 1.0_f64.max(second * 0.2);
    eprintln!(
        "overview scroll comparison: total first/second={:.3}; assembly first/second={:.3}; EGL draw+readback first/second={:.3}; margin={margin:.3} ms; within-margin={}; software EGL draw+readback includes readback cost and is not GPU-only timing",
        first / second.max(f64::EPSILON),
        first_assembly / second_assembly.max(f64::EPSILON),
        first_draw / second_draw.max(f64::EPSILON),
        first <= second + margin,
    );
    if std::env::var_os("NACRE_NIRI_BENCHMARK_STRICT").is_some() {
        assert!(
            first <= second + margin,
            "first scroll exceeded diagnostic margin"
        );
    }
}

#[test]
#[ignore = "manual headless overview frame assembly benchmark"]
fn egl_benchmark_overview_frame_assembly() {
    let mut fixture = fixture();
    let client = fixture.add_client();
    for idx in 0..8 {
        create_window(&mut fixture, client, 900 + idx * 10, 650 + idx * 5);
        if idx % 2 == 1 && idx < 7 {
            fixture.niri().layout.switch_workspace_down();
        }
    }
    let output2 = fixture.niri_output(2);
    fixture.niri().layout.focus_output(&output2);
    for idx in 0..8 {
        create_window(&mut fixture, client, 1000 + idx * 10, 700 + idx * 5);
        if idx % 2 == 1 && idx < 7 {
            fixture.niri().layout.switch_workspace_down();
        }
    }
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
    fixture.niri().layout.create_collection(None);
    create_window(&mut fixture, client, 760, 560);
    fixture
        .niri()
        .layout
        .focus_collection(&niri_config::WorkspaceReference::Id(output1_collection));
    fixture.niri_complete_animations();
    set_time(fixture.niri(), Duration::ZERO);
    fixture.niri().layout.open_overview();
    measure_opening_sweep(&mut fixture);
    for output_idx in [1, 2] {
        measure(&mut fixture, output_idx, "steady");
    }
}
