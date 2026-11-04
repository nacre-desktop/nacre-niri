use std::time::Duration;

use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
use smithay::utils::Point;

use super::client::bead_layer_motion::v1::client::bead_layer_motion_v1::{
    self, Curve, OpacityMode, RevealMode,
};
use super::client::{ClientId, LayerConfigureProps};
use super::fixture::Fixture;

fn setup_panel(f: &mut Fixture) -> (ClientId, wayland_client::protocol::wl_surface::WlSurface) {
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let layer = f.client(id).create_layer(None, Layer::Top, "motion-test");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((100, 100)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);
    (id, surface)
}

fn mapped_surface(
    f: &Fixture,
) -> smithay::reexports::wayland_server::protocol::wl_surface::WlSurface {
    let niri = &f.state.server.state.niri;
    niri.mapped_layer_surfaces
        .keys()
        .find(|layer| layer.namespace() == "motion-test")
        .unwrap()
        .wl_surface()
        .clone()
}

fn create_plane(controller: &bead_layer_motion_v1::BeadLayerMotionV1, from_x: f64, to_x: f64) {
    controller.add_plane(
        7, 0., 0., 100., 100., from_x, 0., 100., 100., 0., 0., 100., 100., 1.,
    );
    controller.set_plane_to(7, to_x, 0., 100., 100., 0., 0., 100., 100., 1.);
    controller.set_curve(Curve::Linear);
    controller.set_plane_opacity_mode(7, OpacityMode::Linear, 0);
}

#[test]
fn controller_arms_on_surface_commit_retargets_and_ignores_stale_cancel() {
    let mut f = Fixture::new();
    let (id, client_surface) = setup_panel(&mut f);
    let motion_proxy = {
        let client = f.client(id);
        client.state.layer_motion(&client_surface).motion.clone()
    };
    create_plane(&motion_proxy, -100., 0.);
    motion_proxy.animate(10, 100, 0);

    let layer = f.client(id).layer(&client_surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let root_surface = mapped_surface(&f);
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
    assert!((runtime.lock().unwrap().current[0].1.destination.loc.x + 60.).abs() < 0.01);
    f.niri().clock.set_rate(0.5);
    f.niri().clock.set_unadjusted(mid);
    let mapped = f.niri().mapped_layer_surfaces.values().next().unwrap();
    assert_eq!(
        mapped.motion_poses().unwrap()[0].1.destination.loc.x,
        runtime.lock().unwrap().current[0].1.destination.loc.x
    );
    assert_eq!(
        mapped.motion_input_point(Point::from((20., 10.))),
        Some(Some(Point::from((80., 10.))))
    );

    let output = f.niri_output(1);
    f.niri()
        .output_state
        .get_mut(&output)
        .unwrap()
        .frame_clock
        .presented(mid);
    motion_proxy.set_plane_from(7, 1000., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.set_plane_to(7, -100., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.animate(11, 100, 1);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);

    {
        let runtime_state = runtime.lock().unwrap();
        let retargeted = runtime_state.active.as_ref().unwrap();
        assert_eq!(retargeted.serial, 11);
        let retarget_progress = (retargeted.start_time() - start).as_secs_f64() / 0.1;
        let expected_x = -100. + 100. * retarget_progress.min(1.);
        assert!(
            (retargeted.sample(retargeted.start_time())[0]
                .1
                .destination
                .loc
                .x
                - expected_x)
                .abs()
                < 0.01
        );
    }

    motion_proxy.cancel(10);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    assert_eq!(runtime.lock().unwrap().active.as_ref().unwrap().serial, 11);

    f.niri()
        .layer_motion_manager_state
        .tick(&root_surface, mid + Duration::from_millis(101));
    f.roundtrip(id);
    let controller = f
        .client(id)
        .state
        .motions
        .iter()
        .find(|controller| controller.motion == motion_proxy)
        .unwrap();
    assert!(controller
        .events
        .iter()
        .any(|event| matches!(event, bead_layer_motion_v1::Event::Cancelled { serial: 10 })));
    assert!(controller
        .events
        .iter()
        .any(|event| matches!(event, bead_layer_motion_v1::Event::Done { serial: 11 })));
}

#[test]
fn reveal_samples_geometry_clip_and_tracking_mode_across_reopen_retarget() {
    let mut f = Fixture::new();
    let (id, client_surface) = setup_panel(&mut f);
    let motion_proxy = {
        let client = f.client(id);
        client.state.layer_motion(&client_surface).motion.clone()
    };
    create_plane(&motion_proxy, -100., 0.);
    motion_proxy.set_reveal_bounds(0., 0., 100., 100., 0., 0., 100., 100.);
    motion_proxy.set_reveal_geometry(
        10.,
        10.,
        0.,
        0.,
        10.,
        10.,
        50.,
        50.,
        0.,
        0.,
        0.,
        0.,
        12.,
        12.,
        12.,
        12.,
        bead_layer_motion_v1::CornerMode::QuadraticCut,
        0,
        0x336699ff,
        7,
    );
    motion_proxy.set_reveal_content_clip(10., 10., 0., 0., 10., 10., 50., 50.);
    motion_proxy.set_reveal_mode(RevealMode::Visible);
    motion_proxy.animate(10, 100, 0);

    let layer = f.client(id).layer(&client_surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    let root_surface = mapped_surface(&f);
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
    let middle = start + Duration::from_millis(50);
    let output = f.niri_output(1);
    f.niri()
        .output_state
        .get_mut(&output)
        .unwrap()
        .frame_clock
        .presented(middle);
    f.niri().clock.set_unadjusted(middle);
    f.state
        .server
        .state
        .niri
        .layer_motion_manager_state
        .tick(&root_surface, middle);
    {
        let runtime = runtime.lock().unwrap();
        let sampled = runtime.reveal_current.unwrap();
        assert_eq!(
            sampled.body,
            super::super::layer::reveal::Rect::new(10., 10., 25., 25.)
        );
        assert_eq!(sampled.radii, [6.; 4]);
        assert_eq!(
            sampled.content_clip.unwrap().w,
            25.,
            "descriptor={:?}, active={:?}",
            runtime.reveal,
            runtime.active
        );
    }

    motion_proxy.set_reveal_mode(RevealMode::TrackingOnly);
    motion_proxy.set_reveal_geometry(
        10.,
        10.,
        25.,
        25.,
        10.,
        10.,
        0.,
        0.,
        6.,
        6.,
        6.,
        6.,
        0.,
        0.,
        0.,
        0.,
        bead_layer_motion_v1::CornerMode::QuadraticCut,
        0,
        0,
        7,
    );
    motion_proxy.set_reveal_content_clip(10., 10., 25., 25., 10., 10., 0., 0.);
    motion_proxy.set_plane_from(7, 0., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.set_plane_to(7, -100., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.animate(11, 100, 1);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    let closing_start = runtime
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .start_time();
    assert!(!runtime.lock().unwrap().reveal.as_ref().unwrap().visible);
    let closing_pose = runtime.lock().unwrap().reveal_current.unwrap().body;
    assert!(closing_pose.w > 0. && closing_pose.w < 50.);
    assert!(runtime
        .lock()
        .unwrap()
        .reveal
        .as_ref()
        .unwrap()
        .contains((80., 80.), 0.));
    let closed = closing_start + Duration::from_millis(100);
    let output = f.niri_output(1);
    f.niri()
        .output_state
        .get_mut(&output)
        .unwrap()
        .frame_clock
        .presented(closed);
    f.niri().clock.set_unadjusted(closed);
    f.state
        .server
        .state
        .niri
        .layer_motion_manager_state
        .tick(&root_surface, closed);
    assert_eq!(
        runtime.lock().unwrap().reveal_current.unwrap().body,
        super::super::layer::reveal::Rect::new(10., 10., 0., 0.)
    );

    motion_proxy.set_reveal_mode(RevealMode::Visible);
    motion_proxy.set_reveal_geometry(
        80.,
        80.,
        0.,
        0.,
        80.,
        80.,
        50.,
        50.,
        0.,
        0.,
        0.,
        0.,
        8.,
        8.,
        8.,
        8.,
        bead_layer_motion_v1::CornerMode::QuadraticCut,
        0,
        0x336699ff,
        7,
    );
    motion_proxy.set_reveal_content_clip(0., 0., 100., 100., 80., 80., 50., 50.);
    motion_proxy.set_plane_from(7, -100., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.set_plane_to(7, 0., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.animate(12, 100, 1);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    let runtime = runtime.lock().unwrap();
    let reopening = runtime.reveal.as_ref().unwrap().sample(0.).body;
    assert_eq!(
        reopening,
        super::super::layer::reveal::Rect::new(10., 10., 0., 0.),
        "reopen descriptor starts at the exact previously closed body"
    );
    assert!(runtime.reveal.as_ref().unwrap().visible);
}

#[test]
fn explicit_cancel_and_later_surface_commit_preserve_sampled_pose() {
    let mut f = Fixture::new();
    let (id, client_surface) = setup_panel(&mut f);
    let motion_proxy = {
        let client = f.client(id);
        client.state.layer_motion(&client_surface).motion.clone()
    };
    create_plane(&motion_proxy, -100., 0.);
    motion_proxy.animate(30, 100, 0);

    let layer = f.client(id).layer(&client_surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);

    let root_surface = mapped_surface(&f);
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

    motion_proxy.cancel(30);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);

    let cancelled_x = runtime.lock().unwrap().current[0].1.destination.loc.x;
    assert!(cancelled_x < -1. && cancelled_x > -100.);
    assert!(runtime.lock().unwrap().active.is_none());

    // An unrelated root-surface commit must not replace the frozen sampled
    // pose with the old target endpoint.
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    assert_eq!(
        runtime.lock().unwrap().current[0].1.destination.loc.x,
        cancelled_x
    );
    let mapped = f.niri().mapped_layer_surfaces.values().next().unwrap();
    assert_eq!(
        mapped.motion_poses().unwrap()[0].1.destination.loc.x,
        cancelled_x
    );

    let controller = f
        .client(id)
        .state
        .motions
        .iter()
        .find(|controller| controller.motion == motion_proxy)
        .unwrap();
    assert!(controller
        .events
        .iter()
        .any(|event| matches!(event, bead_layer_motion_v1::Event::Cancelled { serial: 30 })));
    let event_x = controller.events.iter().find_map(|event| match event {
        bead_layer_motion_v1::Event::PlaneState { plane: 7, x, .. } => Some(*x),
        _ => None,
    });
    assert!(
        event_x.is_some_and(|x| (x - cancelled_x).abs() < 1. / 256.),
        "cancel event pose {event_x:?} differs from frozen runtime pose {cancelled_x}"
    );

    // Retargeting must sample the frozen pose even when the caller supplies a
    // deliberately unrelated from-state.
    motion_proxy.set_plane_from(7, 1000., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.set_plane_to(7, 50., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.animate(31, 100, 1);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    let retargeted = runtime.lock().unwrap().active.clone().unwrap();
    assert_eq!(retargeted.serial, 31);
    assert!(
        (retargeted.sample(retargeted.start_time())[0]
            .1
            .destination
            .loc
            .x
            - cancelled_x)
            .abs()
            < 0.01
    );
}

#[test]
fn unmap_cancels_active_motion_and_controller_can_be_reused_after_remap() {
    let mut f = Fixture::new();
    let (id, client_surface) = setup_panel(&mut f);
    let motion_proxy = {
        let client = f.client(id);
        client.state.layer_motion(&client_surface).motion.clone()
    };
    create_plane(&motion_proxy, 0., 0.);
    motion_proxy.set_reveal_bounds(0., 0., 100., 100., 0., 0., 100., 100.);
    motion_proxy.set_reveal_geometry(
        0.,
        0.,
        0.,
        0.,
        0.,
        0.,
        100.,
        100.,
        0.,
        0.,
        0.,
        0.,
        12.,
        12.,
        12.,
        12.,
        bead_layer_motion_v1::CornerMode::Circular,
        0,
        0,
        7,
    );
    motion_proxy.set_reveal_content_clip(0., 0., 100., 100., 30., 40., 40., 30.);
    motion_proxy.set_reveal_mode(RevealMode::Visible);
    let terminal_bytes = vec![17, 29, 43, 255].repeat(100 * 100);
    let terminal_buffer = f
        .client(id)
        .create_rgba_shm_buffer(100, 100, &terminal_bytes);
    motion_proxy.set_reveal_terminal_buffer(Some(&terminal_buffer), 100., 100.);
    motion_proxy.animate(20, 150, 0);
    let releases_before_terminal_commit = f.client(id).state.released_buffers;

    let layer = f.client(id).layer(&client_surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    assert!(
        f.client(id).state.released_buffers > releases_before_terminal_commit,
        "compositor releases copied terminal SHM buffer"
    );
    let root_surface = mapped_surface(&f);
    let runtime = f
        .state
        .server
        .state
        .niri
        .layer_motion_manager_state
        .runtime(&root_surface)
        .unwrap();
    {
        let runtime = runtime.lock().unwrap();
        let terminal = runtime
            .terminal
            .as_ref()
            .expect("commit copies terminal SHM");
        assert_eq!(terminal.pixels.as_ref(), terminal_bytes.as_slice());
        assert!(!runtime.terminal_ready);
    }
    let start = runtime
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .start_time();
    assert!(runtime.lock().unwrap().active.is_some());
    let midpoint = start + Duration::from_millis(75);
    let output = f.niri_output(1);
    f.niri()
        .output_state
        .get_mut(&output)
        .unwrap()
        .frame_clock
        .presented(midpoint);
    f.niri().clock.set_unadjusted(midpoint);

    f.client(id).layer(&client_surface).attach_null();
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    let frozen_clip = {
        let runtime = runtime.lock().unwrap();
        assert!(runtime.active.is_none());
        assert!(
            runtime.terminal.is_none(),
            "unmap drops retained terminal pixels"
        );
        assert!(!runtime.terminal_ready);
        let frozen_clip = runtime.reveal_current.unwrap().content_clip.unwrap();
        assert_eq!(
            runtime.reveal.as_ref().unwrap().content_clip,
            Some((frozen_clip, frozen_clip)),
            "unmap freezes both content-clip endpoints at the sampled presentation"
        );
        frozen_clip
    };
    let controller = f
        .client(id)
        .state
        .motions
        .iter()
        .find(|controller| controller.motion == motion_proxy)
        .unwrap();
    assert!(controller
        .events
        .iter()
        .any(|event| matches!(event, bead_layer_motion_v1::Event::Cancelled { serial: 20 })));

    motion_proxy.set_plane_from(7, -80., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.set_plane_to(7, 0., 0., 100., 100., 0., 0., 100., 100., 1.);
    motion_proxy.set_reveal_content_clip(15., 20., 70., 65., 50., 60., 20., 20.);
    motion_proxy.animate(21, 100, 1);
    let layer = f.client(id).layer(&client_surface);
    motion_proxy.set_reveal_terminal_buffer(Some(&terminal_buffer), 100., 100.);
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((101, 101)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);
    let layer = f.client(id).layer(&client_surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    let runtime_state = runtime.lock().unwrap();
    assert_eq!(runtime_state.active.as_ref().unwrap().serial, 21);
    assert!(runtime_state.terminal.is_some());
    assert!(!runtime_state.terminal_ready);
    assert_eq!(
        runtime_state
            .reveal
            .as_ref()
            .unwrap()
            .sample(0.)
            .content_clip,
        Some(frozen_clip),
        "remapped retarget starts at the frozen content clip"
    );
    drop(runtime_state);

    // A later data-only endpoint replacement after cancellation must not turn
    // the frozen partial pose into a completed reveal.
    motion_proxy.cancel(21);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    assert!(!runtime.lock().unwrap().terminal_ready);
    let replacement_bytes = vec![53, 41, 31, 255].repeat(100 * 100);
    let replacement = f
        .client(id)
        .create_rgba_shm_buffer(100, 100, &replacement_bytes);
    motion_proxy.set_reveal_terminal_buffer(Some(&replacement), 100., 100.);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    {
        let runtime = runtime.lock().unwrap();
        let terminal = runtime.terminal.as_ref().expect("replacement is retained");
        assert_eq!(terminal.pixels.as_ref(), replacement_bytes.as_slice());
        assert!(!runtime.terminal_ready);
    }

    // A stale animation serial cannot install its terminal buffer. The pending
    // buffer is released at commit and the newer snapshot remains installed.
    let stale_bytes = vec![71, 73, 79, 255].repeat(100 * 100);
    let stale_buffer = f.client(id).create_rgba_shm_buffer(100, 100, &stale_bytes);
    motion_proxy.animate(20, 100, 1);
    motion_proxy.set_reveal_terminal_buffer(Some(&stale_buffer), 100., 100.);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    {
        let runtime = runtime.lock().unwrap();
        assert_eq!(
            runtime.terminal.as_ref().unwrap().pixels.as_ref(),
            replacement_bytes.as_slice()
        );
        assert!(!runtime.terminal_ready);
    }

    motion_proxy.animate(22, 100, 1);
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    let completion = runtime
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .start_time()
        + Duration::from_millis(101);
    f.niri()
        .layer_motion_manager_state
        .tick(&root_surface, completion);
    assert!(runtime.lock().unwrap().terminal_ready);
    motion_proxy.reset();
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    let runtime = runtime.lock().unwrap();
    assert!(runtime.terminal.is_none(), "reset clears terminal pixels");
    assert!(!runtime.terminal_ready);
}
