use niri_config::Config;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::backend::renderer::{ContextId, Renderer as _};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::with_states;

use super::client::ClientId;
use super::Fixture;
use crate::render_helpers::{RenderCtx, RenderTarget};

fn fixture() -> Fixture {
    let config = Config::parse_mem(
        r#"
output "headless-1" { scale 1.5; }
workspace "desk-0" { open-on-output "headless-1"; }
workspace "desk-1" { open-on-output "headless-1"; }
workspace "desk-2" { open-on-output "headless-1"; }
workspace "desk-3" { open-on-output "headless-1"; }
workspace "desk-4" { open-on-output "headless-1"; }
"#,
    )
    .unwrap();
    let mut fixture = Fixture::with_config(config);
    fixture
        .niri_state()
        .backend
        .headless()
        .add_renderer()
        .unwrap();
    fixture.add_output(1, (1200, 800));
    fixture.niri().layout.switch_workspace(2);
    fixture.niri_complete_animations();
    fixture
}

fn create_window(fixture: &mut Fixture, client: ClientId, width: u16, height: u16) -> WlSurface {
    let surface = {
        let window = fixture.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    fixture.roundtrip(client);
    let window = fixture.client(client).window(&surface);
    window.attach_shm_buffer(width, height);
    window.set_size(width, height);
    window.ack_last_and_commit();
    fixture.double_roundtrip(client);
    fixture
        .niri()
        .layout
        .windows()
        .next()
        .expect("mapped client window")
        .1
        .toplevel()
        .wl_surface()
        .clone()
}

fn has_renderer_texture(surface: &WlSurface, context: ContextId<GlesTexture>) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<RendererSurfaceStateUserData>()
            .is_some_and(|data| data.lock().unwrap().texture(context).is_some())
    })
}

fn prepare(fixture: &mut Fixture, output: &Output, target: RenderTarget) {
    fixture.niri().layout.update_render_elements(Some(output));
    render_prewarm(fixture, output, target);
}

fn render_prewarm(fixture: &mut Fixture, output: &Output, target: RenderTarget) {
    let state = &mut fixture.state.server.state;
    let layout = &state.niri.layout;
    state
        .backend
        .with_primary_renderer(|renderer| {
            layout.prewarm_overview(
                RenderCtx {
                    renderer,
                    target,
                    scale_override: None,
                    xray: None,
                },
                output,
            )
        })
        .expect("headless EGL renderer");
}

fn counts(fixture: &Fixture) -> (u64, u64, u64) {
    fixture
        .state
        .server
        .state
        .niri
        .layout
        .active_overview_prewarm_counts()
}

#[test]
fn egl_overview_prewarm_is_one_task_per_frame_and_bounded_to_open_neighbors() {
    let mut fixture = fixture();
    let output = fixture.niri_output(1);
    fixture.niri().layout.open_overview();
    fixture.niri().layout.update_render_elements(Some(&output));
    let total = fixture
        .niri()
        .layout
        .active_overview_prewarm_candidate_count(&output);
    assert!((1..=4).contains(&total), "captured candidates: {total}");

    for expected in 1..=total as u64 {
        prepare(&mut fixture, &output, RenderTarget::Output);
        let (scheduled, attempts, successes) = counts(&fixture);
        assert_eq!(scheduled, expected);
        assert_eq!(attempts, expected);
        assert_eq!(successes, expected);
        assert_eq!(
            fixture.niri().layout.overview_prewarm_pending(&output),
            expected < total as u64
        );
    }

    prepare(&mut fixture, &output, RenderTarget::Output);
    assert_eq!(counts(&fixture), (total as u64, total as u64, total as u64));
    let before_reopen = counts(&fixture);

    fixture.niri().layout.close_overview();
    fixture.niri().layout.open_overview();
    prepare(&mut fixture, &output, RenderTarget::Output);
    assert_eq!(
        counts(&fixture),
        (
            before_reopen.0 + 1,
            before_reopen.1 + 1,
            before_reopen.2 + 1
        )
    );
}

#[test]
fn egl_overview_prewarm_skips_capture_and_never_attaches_a_removed_output() {
    let mut fixture = fixture();
    let output = fixture.niri_output(1);
    fixture.niri().layout.open_overview();
    fixture.niri().layout.update_render_elements(Some(&output));

    prepare(&mut fixture, &output, RenderTarget::ScreenCapture);
    assert_eq!(counts(&fixture), (1, 0, 0));
    assert!(fixture.niri().layout.overview_prewarm_pending(&output));

    fixture.niri().remove_output(&output);
    assert!(fixture.niri().layout.monitor_for_output(&output).is_none());
    render_prewarm(&mut fixture, &output, RenderTarget::Output);
    assert!(fixture.niri().layout.monitor_for_output(&output).is_none());
    assert_eq!(counts(&fixture), (1, 0, 0));
}

#[test]
fn egl_overview_prewarm_imports_an_uncached_offscreen_client_surface() {
    let config = Config::parse_mem(
        r#"
output "headless-1" { scale 1.5; }
workspace "desk-0" { open-on-output "headless-1"; }
workspace "desk-1" { open-on-output "headless-1"; }
workspace "desk-2" { open-on-output "headless-1"; }
workspace "desk-3" { open-on-output "headless-1"; }
workspace "desk-4" { open-on-output "headless-1"; }
"#,
    )
    .unwrap();
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1200, 800));
    let client = fixture.add_client();
    fixture.niri().layout.switch_workspace(3);
    let surface = create_window(&mut fixture, client, 700, 500);
    fixture.niri().layout.switch_workspace(2);
    fixture.niri_complete_animations();

    // Creating the EGL renderer after mapping the window ensures this surface has never been
    // imported into its context before the overview prewarm path runs.
    fixture
        .niri_state()
        .backend
        .headless()
        .add_renderer()
        .unwrap();
    let output = fixture.niri_output(1);
    fixture.niri().layout.open_overview();
    let context = fixture
        .niri_state()
        .backend
        .with_primary_renderer(|renderer| renderer.context_id())
        .expect("headless EGL renderer");
    assert!(!has_renderer_texture(&surface, context.clone()));

    for _ in 0..4 {
        prepare(&mut fixture, &output, RenderTarget::Output);
        if has_renderer_texture(&surface, context.clone()) {
            return;
        }
    }

    panic!("offscreen adjacent workspace surface was not imported by prewarm");
}
