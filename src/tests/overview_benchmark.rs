use std::time::{Duration, Instant};

use niri_config::Config;
use smithay::backend::allocator::Fourcc;
use smithay::utils::{Scale, Transform};

use super::Fixture;
use crate::niri::{Niri, OutputRenderElements};
use crate::render_helpers::{render_to_vec as render_pixels, RenderCtx, RenderTarget};

fn fixture() -> Fixture {
    let mut config = Config::parse_mem(
        "output \"headless-1\" { scale 1.5 }\noutput \"headless-2\" { scale 1.0 }",
    )
    .unwrap();
    config.layout.collections.enabled = true;
    let mut fixture = Fixture::with_config(config);
    fixture.niri_state().backend.headless().add_renderer().unwrap();
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
    window.attach_new_buffer();
    window.set_size(w, h);
    window.ack_last_and_commit();
    fixture.double_roundtrip(client);
}

fn output_elements(fixture: &mut Fixture, output_idx: u8) -> Vec<OutputRenderElements<smithay::backend::renderer::gles::GlesRenderer>> {
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
        set_time(
            fixture.niri(),
            Duration::from_millis(frame * 1000 / 60),
        );
        fixture.niri().advance_animations();
        for output_idx in [1, 2] {
            let (assembly_ms, draw_ms) = sample_frame(fixture, output_idx);
            assembly[usize::from(output_idx - 1)].push(assembly_ms);
            draw[usize::from(output_idx - 1)].push(draw_ms);
        }
    }
    for output_idx in [1, 2] {
        let idx = usize::from(output_idx - 1);
        assembly[idx].sort_by(f64::total_cmp);
        draw[idx].sort_by(f64::total_cmp);
        eprintln!(
            "overview output {output_idx} opening sweep (60 changing frames): assembly median={:.3} p95={:.3} ms; EGL draw+readback median={:.3} p95={:.3} ms",
            assembly[idx][30],
            assembly[idx][57],
            draw[idx][30],
            draw[idx][57]
        );
    }
}

#[test]
#[ignore = "manual headless overview frame assembly benchmark"]
fn benchmark_overview_frame_assembly() {
    let mut fixture = fixture();
    let client = fixture.add_client();
    for idx in 0..8 {
        create_window(&mut fixture, client, 900 + idx * 10, 650 + idx * 5);
        if idx % 2 == 1 {
            fixture.niri().layout.switch_workspace_down();
        }
    }
    let output2 = fixture.niri_output(2);
    fixture.niri().layout.focus_output(&output2);
    for idx in 0..8 {
        create_window(&mut fixture, client, 1000 + idx * 10, 700 + idx * 5);
        if idx % 2 == 1 {
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
    fixture.niri().layout.focus_collection(
        &niri_config::WorkspaceReference::Id(output1_collection),
    );
    set_time(fixture.niri(), Duration::ZERO);
    fixture.niri().layout.open_overview();
    measure_opening_sweep(&mut fixture);
    for output_idx in [1, 2] {
        measure(&mut fixture, output_idx, "steady");
    }
}
