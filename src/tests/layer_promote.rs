//! Protocol and incremental raster proofs for single-owner promotion.
use std::time::Duration;

use niri_config::{Config, WorkspaceReference};
use smithay::backend::allocator::Fourcc;
use smithay::backend::input::ButtonState;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::{Element as _, Id, RenderElement};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind as _, Color32F, ExportMem as _};
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::output::Output;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
use smithay::utils::{Point, SERIAL_COUNTER};
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::bead_layer_promote::v1::client::bead_layer_promote_v1::{
    self, BeadLayerPromoteV1,
};
use super::client::{ClientId, LayerConfigureProps};
use super::Fixture;
use crate::render_helpers::{copy_framebuffer, create_texture, RenderCtx, RenderTarget};

type ServerSurface = smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

fn panel(f: &mut Fixture) -> (ClientId, WlSurface, ServerSurface, BeadLayerPromoteV1) {
    panel_on_output(f, None, (u32::MAX / 2, 0, 0, u32::MAX / 2))
}

fn panel_on_output(
    f: &mut Fixture,
    output_name: Option<&str>,
    color: (u32, u32, u32, u32),
) -> (ClientId, WlSurface, ServerSurface, BeadLayerPromoteV1) {
    let namespace = format!("promote-test-{}", f.state.clients.len());
    let id = f.add_client();
    if output_name.is_some() {
        f.double_roundtrip(id);
    }
    let output = output_name.map(|name| f.client(id).output(name));
    let layer = f
        .client(id)
        .create_layer(output.as_ref(), Layer::Top, &namespace);
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((96, 72)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);
    let proxy = f
        .client(id)
        .state
        .layer_promotion(&surface)
        .promotion
        .clone();
    let layer = f.client(id).layer(&surface);
    let buffer =
        layer
            .spbm
            .create_u32_rgba_buffer(color.0, color.1, color.2, color.3, &layer.qh, ());
    layer.surface.attach(Some(&buffer), 0, 0);
    layer.set_size(96, 72);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    let root = f
        .niri()
        .mapped_layer_surfaces
        .keys()
        .find(|l| l.namespace() == namespace)
        .unwrap()
        .wl_surface()
        .clone();
    (id, surface, root, proxy)
}

fn raster_pixels<E: RenderElement<GlesRenderer>>(
    renderer: &mut GlesRenderer,
    output: &Output,
    texture: &mut GlesTexture,
    elements: &[E],
) -> Vec<u8> {
    let mut target = renderer.bind(texture).unwrap();
    OutputDamageTracker::from_output(output)
        .render_output(renderer, &mut target, 0, elements, Color32F::TRANSPARENT)
        .unwrap();
    let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
    renderer.map_texture(&mapping).unwrap().to_vec()
}

fn pixel_at(bytes: &[u8], width: usize, x: usize, y: usize) -> &[u8] {
    let start = (y * width + x) * 4;
    &bytes[start..start + 4]
}
fn commit(f: &mut Fixture, id: ClientId, surface: &WlSurface) {
    f.client(id).layer(surface).commit();
    f.double_roundtrip(id);
}
fn promote(proxy: &BeadLayerPromoteV1, serial: u32) {
    proxy.promote(serial, None, 0, 8., 4., 40., 30.);
}

#[test]
fn promote_dock_cancel_and_stale_requests_are_commit_atomic_and_keep_identity() {
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    promote(&proxy, 10);
    f.double_roundtrip(id);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
    commit(&mut f, id, &surface);
    let p = &f.niri().layer_promote_manager_state.promoted[&root];
    assert_eq!(p.layer.wl_surface(), &root);
    assert_eq!(p.body_origin, Point::from((8., 4.)));
    proxy.dock(9); // stale even though delivered on a fresh root commit
    commit(&mut f, id, &surface);
    assert!(f.niri().layer_promote_manager_state.is_promoted(&root));
    proxy.cancel(11);
    f.double_roundtrip(id);
    assert!(f.niri().layer_promote_manager_state.is_promoted(&root));
    commit(&mut f, id, &surface);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
    assert!(f
        .niri()
        .mapped_layer_surfaces
        .keys()
        .any(|l| l.wl_surface() == &root));
    promote(&proxy, 12);
    commit(&mut f, id, &surface);
    proxy.dock(13);
    commit(&mut f, id, &surface);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
    let events = &f.client(id).state.promotions[0].events;
    assert!(events.iter().any(|event| matches!(
        event,
        bead_layer_promote_v1::Event::Configure {
            serial: 13,
            state: wayland_client::WEnum::Value(bead_layer_promote_v1::State::Docked),
            ..
        }
    )));
}

#[test]
fn real_button_grab_moves_immediately_and_release_preserves_content() {
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    let seat = f.client(id).state.seat.clone().unwrap();
    let pointer = f.niri().seat.get_pointer().unwrap();
    let serial = SERIAL_COUNTER.next_serial();
    let state = f.niri_state();
    pointer.motion(
        state,
        Some((root.clone(), Point::from((0., 0.)))),
        &MotionEvent {
            location: Point::from((20., 20.)),
            serial,
            time: 1,
        },
    );
    pointer.button(
        state,
        &ButtonEvent {
            serial,
            time: 1,
            button: 0x110,
            state: ButtonState::Pressed,
        },
    );
    // Travel before the client promotes must be included without losing hotspot.
    pointer.motion(
        state,
        Some((root.clone(), Point::from((0., 0.)))),
        &MotionEvent {
            location: Point::from((30., 25.)),
            serial: SERIAL_COUNTER.next_serial(),
            time: 2,
        },
    );
    proxy.promote(1, Some(&seat), serial.into(), 8., 4., 40., 30.);
    commit(&mut f, id, &surface);
    assert_eq!(
        f.niri().layer_promote_manager_state.promoted[&root].body_origin,
        Point::from((18., 9.))
    );
    let state = f.niri_state();
    pointer.motion(
        state,
        None,
        &MotionEvent {
            location: Point::from((70., 55.)),
            serial: SERIAL_COUNTER.next_serial(),
            time: 3,
        },
    );
    pointer.frame(state);
    assert_eq!(
        state.niri.layer_promote_manager_state.promoted[&root].body_origin,
        Point::from((58., 39.))
    );
    pointer.button(
        state,
        &ButtonEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: 4,
            button: 0x110,
            state: ButtonState::Released,
        },
    );
    pointer.frame(state);
    assert!(!state.niri.layer_promote_manager_state.promoted[&root].moving);
    assert!(state
        .niri
        .mapped_layer_surfaces
        .keys()
        .any(|l| l.wl_surface() == &root));
    // Reusing an expired button serial cannot start a second move.
    proxy._move(&seat, serial.into());
    f.double_roundtrip(id);
    assert!(!f.niri().layer_promote_manager_state.promoted[&root].moving);
    assert!(f.client(id).state.promotions[0]
        .events
        .iter()
        .any(|event| matches!(event, bead_layer_promote_v1::Event::Rejected { .. })));
}

#[test]
fn output_migration_and_monitorless_retirement_do_not_leave_input_or_ownership() {
    let mut f =
        Fixture::with_config(Config::parse_mem("output \"headless-2\" { scale 1.5; }").unwrap());
    f.add_output(1, (600, 400));
    f.add_output(2, (900, 600));
    let (id, surface, root, proxy) = panel(&mut f);
    promote(&proxy, 1);
    commit(&mut f, id, &surface);
    let second = f.niri_output(2);
    let at = f
        .niri()
        .global_space
        .output_geometry(&second)
        .unwrap()
        .loc
        .to_f64()
        + Point::from((100., 100.));
    f.niri().move_promoted_layer(&root, at, at);
    assert_eq!(
        f.niri().layer_promote_manager_state.promoted[&root].output,
        second
    );
    assert!(f
        .niri()
        .contents_under(at)
        .surface
        .is_some_and(|(s, _)| s == root));
    let first = f.niri_output(1);
    f.niri().remove_output(&second);
    assert_eq!(
        f.niri().layer_promote_manager_state.promoted[&root].output,
        first
    );
    f.niri().remove_output(&first);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
    // Requests with no monitor remain harmless and never regain ownership.
    proxy.dock(2);
    f.double_roundtrip(id);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
}

#[test]
fn hidden_collection_and_unmap_remove_both_visual_and_input_ownership() {
    let mut config = Config::default();
    config.layout.collections.enabled = true;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    promote(&proxy, 1);
    commit(&mut f, id, &surface);
    let output = f.niri_output(1);
    assert_eq!(f.niri().promoted_layers_for_output(&output).len(), 1);
    let collection = f.niri().layout.create_collection(None);
    f.niri()
        .layout
        .focus_collection(&WorkspaceReference::Id(collection));
    f.niri_complete_animations();
    assert!(f.niri().promoted_layers_for_output(&output).is_empty());
    assert!(f
        .niri()
        .contents_under(Point::from((12., 12.)))
        .surface
        .is_none());
    f.client(id).layer(&surface).surface.attach(None, 0, 0);
    commit(&mut f, id, &surface);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
}

#[test]
fn hidden_incoming_collection_promotion_tracks_swipe_and_cannot_receive_hit_testing() {
    let mut config = Config::parse_mem("output \"headless-1\" { scale 1.5; }").unwrap();
    config.layout.collections.enabled = true;
    let mut f = Fixture::with_config(config);
    f.add_output(1, (600, 450));
    let (id, surface, root, proxy) = panel(&mut f);
    promote(&proxy, 1);
    commit(&mut f, id, &surface);
    let output = f.niri_output(1);
    let original_workspace = f.niri().layer_promote_manager_state.promoted[&root].workspace;

    f.niri().layout.create_collection(Some("incoming".into()));
    assert!(f.niri().layout.collection_switch_gesture_begin(&output));
    f.niri()
        .layout
        .collection_switch_gesture_update(180., Duration::from_millis(10));

    let (incoming, incoming_x) = f
        .niri()
        .layout
        .collection_switch_render_workspace(&output)
        .expect("hidden desk is rendered during the swipe");
    assert_eq!(incoming.id().get(), original_workspace);
    assert!(f
        .niri()
        .promoted_layers_for_output(&output)
        .iter()
        .any(|promotion| promotion.layer.wl_surface() == &root));
    let promotion = f.niri().layer_promote_manager_state.promoted[&root].clone();
    assert!(!f.niri().promotion_can_receive_input(&promotion));
    assert!(!f.niri().layer_promotion_can_receive_input(&root));
    let layer = f
        .niri()
        .mapped_layer_surfaces
        .keys()
        .find(|layer| layer.wl_surface() == &root)
        .unwrap()
        .clone();
    f.niri().layer_shell_on_demand_focus = Some(layer);
    f.niri_state().update_keyboard_focus();
    assert!(f.niri().layer_shell_on_demand_focus.is_none());
    assert_eq!(
        f.niri()
            .layout
            .collection_switch_workspace_x_offset(&output, original_workspace),
        Some(incoming_x)
    );
    assert!(f
        .niri()
        .contents_under(Point::from((20., 20.)))
        .surface
        .is_none());
}

#[test]
fn promoted_on_demand_focus_returns_after_exit_dialog_closes() {
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    promote(&proxy, 1);
    commit(&mut f, id, &surface);
    let layer = f
        .niri()
        .mapped_layer_surfaces
        .keys()
        .find(|layer| layer.wl_surface() == &root)
        .unwrap()
        .clone();
    f.niri().layer_shell_on_demand_focus = Some(layer);
    let promotion = f.niri().layer_promote_manager_state.promoted[&root].clone();
    assert!(f.niri().promotion_workspace_input_eligible(&promotion));

    assert!(f.niri().exit_confirm_dialog.show());
    f.niri_state().update_keyboard_focus();
    assert!(!f.niri().promotion_can_receive_input(&promotion));
    assert!(f.niri().layer_promotion_can_receive_input(&root));
    assert_eq!(
        f.niri().keyboard_focus,
        crate::niri::KeyboardFocus::ExitConfirmDialog
    );
    assert!(f.niri().layer_shell_on_demand_focus.is_some());

    assert!(f.niri().exit_confirm_dialog.hide());
    f.niri_complete_animations();
    f.niri_state().update_keyboard_focus();
    assert_eq!(
        f.niri().keyboard_focus,
        crate::niri::KeyboardFocus::LayerShell { surface: root }
    );
}

#[test]
fn collection_swipe_raster_clips_fitted_promotions_and_repaints_old_pixels() {
    let mut config = Config::parse_mem("output \"headless-1\" { scale 1.5; }").unwrap();
    config.layout.collections.enabled = true;
    let mut f = Fixture::with_config(config);
    // The incoming desk is resident on a different, 1x output while the swipe output is 1.5x.
    f.add_output(2, (900, 600));
    let (incoming_client, incoming_surface, incoming_root, incoming_proxy) = panel_on_output(
        &mut f,
        Some("headless-2"),
        (0, 0, u32::MAX / 2, u32::MAX / 2),
    );
    promote(&incoming_proxy, 1);
    commit(&mut f, incoming_client, &incoming_surface);
    let incoming_workspace =
        f.niri().layer_promote_manager_state.promoted[&incoming_root].workspace;

    f.add_output(1, (600, 450));
    let output = f.niri_output(1);
    f.niri().layout.focus_output(&output);
    let (source_client, source_surface, source_root, source_proxy) = panel_on_output(
        &mut f,
        Some("headless-1"),
        (u32::MAX / 2, 0, 0, u32::MAX / 2),
    );
    promote(&source_proxy, 2);
    commit(&mut f, source_client, &source_surface);
    let source_origin = f
        .niri()
        .global_space
        .output_geometry(&output)
        .unwrap()
        .loc
        .to_f64()
        + Point::from((360., 20.));
    f.niri()
        .move_promoted_layer(&source_root, source_origin, source_origin);

    f.niri_state().backend.headless().add_renderer().unwrap();
    let (mut retained, mut fresh, mut side) = f
        .niri_state()
        .backend
        .with_primary_renderer(|renderer| {
            (
                create_texture(renderer, (600, 450).into(), Fourcc::Abgr8888).unwrap(),
                create_texture(renderer, (600, 450).into(), Fourcc::Abgr8888).unwrap(),
                create_texture(renderer, (600, 450).into(), Fourcc::Abgr8888).unwrap(),
            )
        })
        .unwrap();
    let mut damage = OutputDamageTracker::from_output(&output);
    let source_id = Id::from_wayland_resource(&source_root);
    let incoming_id = Id::from_wayland_resource(&incoming_root);
    let mut baseline = Vec::new();

    f.niri().update_render_elements(Some(&output));
    {
        let state = f.niri_state();
        let niri = &state.niri;
        state
            .backend
            .with_primary_renderer(|renderer| {
                let elements = niri.render_to_vec(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    &output,
                    false,
                );
                let mut target = renderer.bind(&mut retained).unwrap();
                damage
                    .render_output(renderer, &mut target, 0, &elements, Color32F::TRANSPARENT)
                    .unwrap();
                let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
                baseline = renderer.map_texture(&mapping).unwrap().to_vec();
            })
            .unwrap();
    }

    assert!(f.niri().layout.collection_switch_gesture_begin(&output));
    f.niri()
        .layout
        .collection_switch_gesture_update(-150., Duration::from_millis(10));
    assert_eq!(
        f.niri()
            .layout
            .collection_switch_render_workspace(&output)
            .unwrap()
            .0
            .id()
            .get(),
        incoming_workspace
    );
    f.niri().update_render_elements(Some(&output));

    let state = f.niri_state();
    let niri = &state.niri;
    state
        .backend
        .with_primary_renderer(|renderer| {
            let elements = niri.render_to_vec(
                RenderCtx {
                    renderer,
                    target: RenderTarget::Output,
                    xray: None,
                },
                &output,
                false,
            );
            assert!(elements.iter().any(|elem| elem.id() == &source_id));
            assert!(elements.iter().any(|elem| elem.id() == &incoming_id));

            let mut target = renderer.bind(&mut retained).unwrap();
            damage
                .render_output(renderer, &mut target, 1, &elements, Color32F::TRANSPARENT)
                .unwrap();
            let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
            let incremental = renderer.map_texture(&mapping).unwrap().to_vec();
            drop(target);

            let full = niri.render_to_vec(
                RenderCtx {
                    renderer,
                    target: RenderTarget::Output,
                    xray: None,
                },
                &output,
                false,
            );
            let full_pixels = raster_pixels(renderer, &output, &mut fresh, &full);
            assert_eq!(
                incremental, full_pixels,
                "incremental repaint differs from fresh raster"
            );

            let source_only: Vec<_> = niri
                .render_to_vec(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    &output,
                    false,
                )
                .into_iter()
                .filter(|elem| elem.id() != &incoming_id)
                .collect();
            let source_pixels = raster_pixels(renderer, &output, &mut side, &source_only);

            let incoming_only: Vec<_> = niri
                .render_to_vec(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    &output,
                    false,
                )
                .into_iter()
                .filter(|elem| elem.id() != &source_id)
                .collect();
            let incoming_pixels = raster_pixels(renderer, &output, &mut side, &incoming_only);

            let base: Vec<_> = niri
                .render_to_vec(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    &output,
                    false,
                )
                .into_iter()
                .filter(|elem| elem.id() != &source_id && elem.id() != &incoming_id)
                .collect();
            let base_pixels = raster_pixels(renderer, &output, &mut side, &base);

            // At 180 logical px only the outgoing red panel should remain; at 220 only
            // the incoming blue panel should remain. Their desk boundary is x=200.
            assert_eq!(
                pixel_at(&incremental, 600, 270, 45),
                pixel_at(&source_pixels, 600, 270, 45)
            );
            assert_ne!(
                pixel_at(&incremental, 600, 270, 45),
                pixel_at(&base_pixels, 600, 270, 45)
            );
            assert_eq!(
                pixel_at(&incremental, 600, 330, 45),
                pixel_at(&incoming_pixels, 600, 330, 45)
            );
            assert_ne!(
                pixel_at(&incremental, 600, 330, 45),
                pixel_at(&base_pixels, 600, 330, 45)
            );
            assert_ne!(
                pixel_at(&incremental, 600, 270, 45),
                pixel_at(&incremental, 600, 330, 45)
            );
            assert_eq!(
                pixel_at(&source_pixels, 600, 330, 45),
                pixel_at(&base_pixels, 600, 330, 45)
            );
            assert_eq!(
                pixel_at(&incoming_pixels, 600, 270, 45),
                pixel_at(&base_pixels, 600, 270, 45)
            );
            // The outgoing panel's pre-swipe location is cleared after the transition moves it.
            assert_eq!(
                pixel_at(&incremental, 600, 560, 60),
                pixel_at(&base_pixels, 600, 560, 60)
            );
            assert_ne!(
                pixel_at(&baseline, 600, 560, 60),
                pixel_at(&base_pixels, 600, 560, 60)
            );
        })
        .unwrap();
}

#[test]
fn invalid_body_is_rejected_without_changing_the_docked_surface() {
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    proxy.promote(1, None, 0, 80., 60., 40., 30.);
    commit(&mut f, id, &surface);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
    assert!(f.client(id).state.promotions[0]
        .events
        .iter()
        .any(|event| matches!(
            event,
            bead_layer_promote_v1::Event::Rejected { serial: 1, .. }
        )));
}

#[test]
fn egl_handoff_and_move_incremental_damage_match_full_raster_at_fractional_scale() {
    let mut f =
        Fixture::with_config(Config::parse_mem("output \"headless-1\" { scale 1.5; }").unwrap());
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (600, 450));
    let (id, surface, root, proxy) = panel(&mut f);
    let output = f.niri_output(1);
    let (mut retained, mut full) = f
        .niri_state()
        .backend
        .with_primary_renderer(|renderer| {
            (
                create_texture(renderer, (600, 450).into(), Fourcc::Abgr8888).unwrap(),
                create_texture(renderer, (600, 450).into(), Fourcc::Abgr8888).unwrap(),
            )
        })
        .unwrap();
    let mut damage = OutputDamageTracker::from_output(&output);
    let mut baseline = None;
    let mut background = None;
    let root_id = Id::from_wayland_resource(&root);
    let mut saw_partial = false;
    for step in 0..4 {
        match step {
            1 => {
                promote(&proxy, 1);
                commit(&mut f, id, &surface);
            }
            2 => f.niri().move_promoted_layer(
                &root,
                Point::from((188.25, 104.5)),
                Point::from((200., 120.)),
            ),
            3 => {
                proxy.dock(2);
                commit(&mut f, id, &surface);
                let terminal = f
                    .niri()
                    .output_state
                    .get(&output)
                    .unwrap()
                    .frame_clock
                    .next_presentation_time()
                    + Duration::from_secs(1);
                f.niri().clock.set_unadjusted(terminal);
            }
            _ => {}
        }
        f.niri().update_render_elements(Some(&output));
        let state = f.niri_state();
        let niri = &state.niri;
        state
            .backend
            .with_primary_renderer(|renderer| {
                let mut elements = niri.render_to_vec(
                    RenderCtx {
                        renderer,
                        target: RenderTarget::Output,
                        xray: None,
                    },
                    &output,
                    false,
                );
                if step == 0 {
                    // Capture the clean desktop at identical coordinates. Workspace
                    // and outside-workspace colors differ, so another location is
                    // not a valid reference for the old dock pixels.
                    let index = elements.iter().position(|e| e.id() == &root_id).unwrap();
                    let root_element = elements.remove(index);
                    let mut target = renderer.bind(&mut full).unwrap();
                    OutputDamageTracker::from_output(&output)
                        .render_output(renderer, &mut target, 0, &elements, Color32F::TRANSPARENT)
                        .unwrap();
                    let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
                    background = Some(renderer.map_texture(&mapping).unwrap().to_vec());
                    elements.insert(index, root_element);
                }
                let mut target = renderer.bind(&mut retained).unwrap();
                {
                    let result = damage
                        .render_output(
                            renderer,
                            &mut target,
                            usize::from(step > 0),
                            &elements,
                            Color32F::TRANSPARENT,
                        )
                        .unwrap();
                    saw_partial |= result.damage.is_some_and(|rects| {
                        let area: i64 = rects
                            .iter()
                            .map(|r| i64::from(r.size.w) * i64::from(r.size.h))
                            .sum();
                        area > 0 && area < 600 * 450
                    });
                }
                let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
                let actual = renderer.map_texture(&mapping).unwrap().to_vec();
                drop(target);
                let mut target = renderer.bind(&mut full).unwrap();
                OutputDamageTracker::from_output(&output)
                    .render_output(renderer, &mut target, 0, &elements, Color32F::TRANSPARENT)
                    .unwrap();
                let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
                let expected = renderer.map_texture(&mapping).unwrap().to_vec();
                drop(target);
                assert!(
                    actual == expected,
                    "step {step}: stale pixels after incremental repaint"
                );
                for capture in [RenderTarget::ScreenCapture, RenderTarget::Screencast] {
                    let capture_elements = niri.render_to_vec(
                        RenderCtx {
                            renderer,
                            target: capture,
                            xray: None,
                        },
                        &output,
                        false,
                    );
                    let mut capture_target = renderer.bind(&mut full).unwrap();
                    OutputDamageTracker::from_output(&output)
                        .render_output(
                            renderer,
                            &mut capture_target,
                            0,
                            &capture_elements,
                            Color32F::TRANSPARENT,
                        )
                        .unwrap();
                    let mapping =
                        copy_framebuffer(renderer, &capture_target, Fourcc::Abgr8888).unwrap();
                    let captured = renderer.map_texture(&mapping).unwrap();
                    assert!(
                        captured == expected.as_slice(),
                        "step {step}: capture and output ownership differ for {capture:?}"
                    );
                }
                if step == 0 {
                    baseline = Some(actual.clone());
                }
                if step == 1 {
                    assert!(
                        &actual == baseline.as_ref().unwrap(),
                        "handoff frame must preserve every pixel, including alpha"
                    );
                }
                if step == 3 {
                    assert!(
                        &actual == baseline.as_ref().unwrap(),
                        "docked terminal frame must return to the original pixels"
                    );
                }
                if step == 2 {
                    let old = (20 * 600 + 20) * 4;
                    assert_eq!(
                        &actual[old..old + 4],
                        &background.as_ref().unwrap()[old..old + 4],
                        "old layer location must contain only background"
                    );
                }
            })
            .unwrap();
    }
    assert!(
        saw_partial,
        "test must exercise partial damage, not only full repaints"
    );
}

#[test]
fn another_client_cannot_promote_using_someone_elses_pointer_grab() {
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (_, _, first_root, _) = panel(&mut f);
    let (id, surface, root, proxy) = panel(&mut f);
    let seat = f.client(id).state.seat.clone().unwrap();
    let pointer = f.niri().seat.get_pointer().unwrap();
    let serial = SERIAL_COUNTER.next_serial();
    let state = f.niri_state();
    pointer.motion(
        state,
        Some((first_root, Point::from((0., 0.)))),
        &MotionEvent {
            location: Point::from((20., 20.)),
            serial,
            time: 1,
        },
    );
    pointer.button(
        state,
        &ButtonEvent {
            serial,
            time: 1,
            button: 0x110,
            state: ButtonState::Pressed,
        },
    );
    proxy.promote(1, Some(&seat), serial.into(), 8., 4., 40., 30.);
    commit(&mut f, id, &surface);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
    assert!(f.client(id).state.promotions[0]
        .events
        .iter()
        .any(|event| matches!(
            event,
            bead_layer_promote_v1::Event::Rejected { serial: 1, .. }
        )));
}

#[test]
fn promotion_suspends_exclusive_space_and_controller_destruction_restores_it() {
    use smithay::desktop::layer_map_for_output;
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    f.client(id)
        .layer(&surface)
        .set_configure_props(LayerConfigureProps {
            anchor: Some(Anchor::Top),
            exclusive_zone: Some(55),
            ..Default::default()
        });
    commit(&mut f, id, &surface);
    let output = f.niri_output(1);
    assert_eq!(layer_map_for_output(&output).non_exclusive_zone().loc.y, 55);
    promote(&proxy, 1);
    commit(&mut f, id, &surface);
    assert_eq!(layer_map_for_output(&output).non_exclusive_zone().loc.y, 0);
    assert!(f.niri().layer_promote_manager_state.is_promoted(&root));
    proxy.destroy();
    f.double_roundtrip(id);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
    assert_eq!(layer_map_for_output(&output).non_exclusive_zone().loc.y, 55);
    assert!(f
        .niri()
        .mapped_layer_surfaces
        .keys()
        .any(|layer| layer.wl_surface() == &root));
}

#[test]
fn surviving_output_origin_changes_keep_the_same_promoted_surface_visible() {
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    f.add_output(2, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    promote(&proxy, 1);
    commit(&mut f, id, &surface);
    let first = f.niri_output(1);
    let second = f.niri_output(2);
    f.niri().remove_output(&first);
    f.niri().refresh_layout();
    let new_origin = f
        .niri()
        .global_space
        .output_geometry(&second)
        .unwrap()
        .loc
        .to_f64();
    assert_eq!(
        f.niri().layer_promote_manager_state.promoted[&root].body_origin,
        new_origin + Point::from((16., 16.))
    );
    let body = f.niri().layer_promote_manager_state.promoted[&root].body_origin;
    // Exercise a geometry change even if automatic output placement retained
    // the surviving connector's previous position on this configuration.
    f.niri().global_space.map_output(&second, (-80, 20));
    f.niri().refresh_layout();
    let shifted_body = body + Point::from((-80., 20.)) - new_origin;
    assert_eq!(
        f.niri().layer_promote_manager_state.promoted[&root].body_origin,
        shifted_body
    );
    assert!(f
        .niri()
        .contents_under(shifted_body + Point::from((1., 1.)))
        .surface
        .is_some_and(|(s, _)| s == root));
}

#[test]
fn promoted_natural_extent_is_commit_atomic_without_layer_resize_handshake() {
    use smithay::utils::Size;
    use smithay::wayland::compositor::with_states;
    let mut f =
        Fixture::with_config(Config::parse_mem("output \"headless-1\" { scale 1.5; }").unwrap());
    f.add_output(1, (900, 600));
    let (id, surface, root, proxy) = panel(&mut f);
    proxy.promote(1, None, 0, 8., 4., 40., 68.);
    commit(&mut f, id, &surface);
    let origin = f.niri().layer_promote_manager_state.promoted[&root].body_origin;
    proxy.set_body_size(40., 90.);
    f.client(id).layer(&surface).set_size(96, 94);
    f.double_roundtrip(id);
    assert_eq!(
        f.niri().layer_promote_manager_state.promoted[&root]
            .local_body
            .size,
        Size::from((40., 68.))
    );
    commit(&mut f, id, &surface);
    let p = &f.niri().layer_promote_manager_state.promoted[&root];
    assert_eq!(p.local_body.size, Size::from((40., 90.)));
    assert_eq!(p.body_origin, origin);
    assert_eq!(p.serial, 1);
    assert_eq!(p.layer.wl_surface(), &root);
    assert_eq!(
        with_states(&root, crate::utils::surface_geo)
            .unwrap()
            .size
            .h,
        94
    );
    assert_eq!(
        f.client(id)
            .layer(&surface)
            .configures_received
            .last()
            .unwrap()
            .1
            .size
            .1,
        72
    );
    // The extra 22 logical pixels are real content/input, beyond the retained
    // layer-shell configure, on the original root at the original placement.
    assert!(f
        .niri()
        .contents_under(Point::from((20., 89.)))
        .surface
        .is_some_and(|(s, _)| s == root));
    proxy.set_body_size(40., 100.); // does not fit the newly committed texture
    commit(&mut f, id, &surface);
    assert_eq!(
        f.niri().layer_promote_manager_state.promoted[&root]
            .local_body
            .size
            .h,
        90.
    );
    proxy.dock(2);
    commit(&mut f, id, &surface);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
    // A late frame's body update has no effect after docking.
    proxy.set_body_size(40., 90.);
    commit(&mut f, id, &surface);
    assert!(!f.niri().layer_promote_manager_state.is_promoted(&root));
}

#[test]
fn close_window_targets_the_focused_promotion_instead_of_the_window_below() {
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    let window = f.client(id).create_window();
    let window_surface = window.surface.clone();
    window.commit();
    f.double_roundtrip(id);
    let window = f.client(id).window(&window_surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    promote(&proxy, 1);
    commit(&mut f, id, &surface);
    assert_eq!(f.niri().keyboard_focus.surface(), Some(&root));
    f.niri_state()
        .do_action(niri_config::Action::CloseWindow, false);
    f.double_roundtrip(id);
    assert!(f.client(id).layer(&surface).close_requested);
    assert!(!f.client(id).window(&window_surface).close_requested);
}

#[test]
fn controllers_can_be_created_while_a_monitorless_layer_is_closing() {
    let mut f = Fixture::new();
    let id = f.add_client();
    let layer = f
        .client(id)
        .create_layer(None, Layer::Top, "closing-native");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((96, 72)),
        ..Default::default()
    });
    // These requests can already be queued when the client receives close.
    // Both optional controllers validate the live role, not output presence.
    f.client(id).state.layer_promotion(&surface);
    f.client(id).state.layer_motion(&surface);
    f.client(id).layer(&surface).commit();
    f.double_roundtrip(id);
    assert!(f.client(id).layer(&surface).close_requested);
    assert!(f.niri().layer_promote_manager_state.promoted.is_empty());
    assert!(f.niri().mapped_layer_surfaces.is_empty());
    assert!(f.niri().layout.active_output().is_none());
}

#[test]
fn mapped_promotion_preserves_actual_pose_and_first_content_uses_saved_placement() {
    use super::client::LayerMargin;
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    f.client(id)
        .layer(&surface)
        .set_configure_props(LayerConfigureProps {
            margin: Some(LayerMargin {
                top: 60,
                right: 0,
                bottom: 0,
                left: 80,
            }),
            ..Default::default()
        });
    promote(&proxy, 1);
    commit(&mut f, id, &surface);
    assert_eq!(
        f.niri().layer_promote_manager_state.promoted[&root].body_origin,
        Point::from((8., 4.))
    );
    assert!(f
        .niri()
        .contents_under(Point::from((20., 20.)))
        .surface
        .is_some_and(|(s, _)| s == root));
    assert!(f
        .niri()
        .contents_under(Point::from((120., 100.)))
        .surface
        .is_none());

    let layer = f.client(id).create_layer(None, Layer::Top, "restore-first");
    let restored = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top),
        size: Some((96, 72)),
        ..Default::default()
    });
    layer.commit();
    f.double_roundtrip(id);
    let restoration = f
        .client(id)
        .state
        .layer_promotion(&restored)
        .promotion
        .clone();
    restoration.promote(1, None, 0, 8., 4., 40., 30.);
    let layer = f.client(id).layer(&restored);
    layer.set_configure_props(LayerConfigureProps {
        margin: Some(LayerMargin {
            top: 100,
            right: 0,
            bottom: 0,
            left: 120,
        }),
        ..Default::default()
    });
    layer.attach_new_buffer();
    layer.set_size(96, 72);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    let p = f
        .niri()
        .layer_promote_manager_state
        .promoted
        .values()
        .find(|p| p.layer.namespace() == "restore-first")
        .unwrap();
    assert_eq!(p.body_origin, Point::from((128., 104.)));
}

#[test]
fn interactive_promotion_keeps_the_pressed_subsurface_pointer_stream() {
    let mut f = Fixture::new();
    f.add_output(1, (600, 400));
    let (id, surface, root, proxy) = panel(&mut f);
    let client = f.client(id);
    let child = client
        .state
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&client.qh, ());
    let sub = client.state.subcompositor.as_ref().unwrap().get_subsurface(
        &child,
        &surface,
        &client.qh,
        (),
    );
    sub.set_position(13, 9);
    let child_viewport =
        client
            .state
            .viewporter
            .as_ref()
            .unwrap()
            .get_viewport(&child, &client.qh, ());
    child_viewport.set_destination(40, 30);
    let buffer = client.state.spbm.as_ref().unwrap().create_u32_rgba_buffer(
        u32::MAX,
        0,
        0,
        u32::MAX,
        &client.qh,
        (),
    );
    child.attach(Some(&buffer), 0, 0);
    child.commit();
    commit(&mut f, id, &surface);
    let pressed = smithay::wayland::compositor::get_children(&root)
        .into_iter()
        .next()
        .unwrap();
    let seat = f.client(id).state.seat.clone().unwrap();
    let pointer = f.niri().seat.get_pointer().unwrap();
    let serial = SERIAL_COUNTER.next_serial();
    let state = f.niri_state();
    pointer.motion(
        state,
        Some((pressed.clone(), Point::from((13., 9.)))),
        &MotionEvent {
            location: Point::from((20., 20.)),
            serial,
            time: 1,
        },
    );
    pointer.button(
        state,
        &ButtonEvent {
            serial,
            time: 1,
            button: 0x110,
            state: ButtonState::Pressed,
        },
    );
    pointer.frame(state);
    f.double_roundtrip(id);
    f.client(id).state.pointer_events.clear();
    proxy.promote(1, Some(&seat), serial.into(), 8., 4., 40., 30.);
    commit(&mut f, id, &surface);
    let state = f.niri_state();
    pointer.motion(
        state,
        Some((pressed, Point::from((63., 44.)))),
        &MotionEvent {
            location: Point::from((70., 55.)),
            serial: SERIAL_COUNTER.next_serial(),
            time: 2,
        },
    );
    pointer.button(
        state,
        &ButtonEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: 3,
            button: 0x110,
            state: ButtonState::Released,
        },
    );
    pointer.frame(state);
    f.double_roundtrip(id);
    let events = &f.client(id).state.pointer_events;
    use wayland_client::protocol::wl_pointer;
    assert!(
        !events.iter().any(|e| matches!(
            e,
            wl_pointer::Event::Enter { .. } | wl_pointer::Event::Leave { .. }
        )),
        "the held child focus must not change: {events:?}"
    );
    assert!(events.iter().any(|e| matches!(e, wl_pointer::Event::Motion { surface_x, surface_y, .. } if *surface_x == 7. && *surface_y == 11.)), "the pressed child-local hotspot stays fixed: {events:?}");
    assert!(events.iter().any(|e| matches!(
        e,
        wl_pointer::Event::Button {
            state: wayland_client::WEnum::Value(wl_pointer::ButtonState::Released),
            ..
        }
    )));
}
