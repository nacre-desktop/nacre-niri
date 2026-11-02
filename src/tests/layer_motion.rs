use std::time::Duration;

use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;

use super::client::bead_layer_motion::v1::client::bead_layer_motion_v1::{
    self, Curve, OpacityMode,
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

    let runtime_state = runtime.lock().unwrap();
    let retargeted = runtime_state.active.as_ref().unwrap();
    assert_eq!(retargeted.serial, 11);
    assert!((retargeted.sample(mid).first().unwrap().1.destination.loc.x + 60.).abs() < 0.01);

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
fn unmap_cancels_active_motion_and_controller_can_be_reused_after_remap() {
    let mut f = Fixture::new();
    let (id, client_surface) = setup_panel(&mut f);
    let motion_proxy = {
        let client = f.client(id);
        client.state.layer_motion(&client_surface).motion.clone()
    };
    create_plane(&motion_proxy, 0., 0.);
    motion_proxy.animate(20, 150, 0);

    let layer = f.client(id).layer(&client_surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    let root_surface = mapped_surface(&f);
    assert!(f
        .state
        .server
        .state
        .niri
        .layer_motion_manager_state
        .runtime(&root_surface)
        .unwrap()
        .lock()
        .unwrap()
        .active
        .is_some());

    f.client(id).layer(&client_surface).attach_null();
    f.client(id).layer(&client_surface).commit();
    f.double_roundtrip(id);
    let runtime = f
        .state
        .server
        .state
        .niri
        .layer_motion_manager_state
        .runtime(&root_surface)
        .unwrap();
    assert!(runtime.lock().unwrap().active.is_none());
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
    motion_proxy.animate(21, 100, 0);
    let layer = f.client(id).layer(&client_surface);
    layer.attach_new_buffer();
    layer.set_size(100, 100);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    assert_eq!(runtime.lock().unwrap().active.as_ref().unwrap().serial, 21);
}
