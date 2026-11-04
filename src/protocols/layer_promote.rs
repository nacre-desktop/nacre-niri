//! Same-surface floating presentation; the Wayland role never changes.
use std::collections::HashMap;

use smithay::desktop::{layer_map_for_output, LayerSurface, WindowSurfaceType};
use smithay::input::pointer::{Focus, GrabStartData};
use smithay::input::Seat;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Logical, Point, Rectangle, Serial, Size};
use smithay::wayland::compositor::{get_parent, with_states};
use smithay::wayland::shell::wlr_layer::{ExclusiveZone, LayerSurfaceCachedState};

use crate::input::layer_move_grab::LayerMoveGrab;
use crate::niri::{Niri, State};
use crate::protocols::raw::bead_layer_promote::v1::server::bead_layer_promote_manager_v1::{
    self, BeadLayerPromoteManagerV1,
};
use crate::protocols::raw::bead_layer_promote::v1::server::bead_layer_promote_v1::{
    self, BeadLayerPromoteV1,
};
use crate::utils::{is_mapped, output_size, send_scale_transform};

#[derive(Clone)]
pub struct PromotionData(pub WlSurface);

#[derive(Clone, Debug)]
pub struct PromotedLayer {
    pub layer: LayerSurface,
    pub output: Output,
    pub local_body: Rectangle<f64, Logical>,
    pub body_origin: Point<f64, Logical>,
    output_origin: Point<f64, Logical>,
    pub workspace: u64,
    collection: Option<u64>,
    pub moving: bool,
    pub serial: u32,
    origin_output: Output,
    order: u64,
    dock_zone: ExclusiveZone,
}

fn set_exclusive_zone(surface: &WlSurface, zone: ExclusiveZone) {
    with_states(surface, |states| {
        states
            .cached_state
            .get::<LayerSurfaceCachedState>()
            .current()
            .exclusive_zone = zone;
    });
}

#[derive(Clone)]
enum Pending {
    Promote {
        serial: u32,
        seat: Option<WlSeat>,
        button_serial: u32,
        body: Rectangle<f64, Logical>,
    },
    Dock(u32),
    Cancel(u32),
}
struct Controller {
    resource: BeadLayerPromoteV1,
    pending: Option<Pending>,
    last_serial: Option<u32>,
    pending_body_size: Option<Size<f64, Logical>>,
}
#[derive(Default)]
pub struct LayerPromoteManagerState {
    controllers: HashMap<WlSurface, Controller>,
    pub promoted: HashMap<WlSurface, PromotedLayer>,
    order: u64,
}
impl LayerPromoteManagerState {
    pub fn new(display: &DisplayHandle) -> Self {
        display.create_global::<State, BeadLayerPromoteManagerV1, _>(1, ());
        Self::default()
    }
    pub fn is_promoted(&self, surface: &WlSurface) -> bool {
        self.promoted.contains_key(surface)
    }
    fn reject(&self, surface: &WlSurface, serial: u32, reason: &str) {
        if let Some(c) = self.controllers.get(surface) {
            c.resource.rejected(serial, reason.to_owned());
        }
    }
    pub fn remove_surface(&mut self, surface: &WlSurface) {
        if let Some(p) = self.promoted.remove(surface) {
            set_exclusive_zone(surface, p.dock_zone);
        }
        if let Some(c) = self.controllers.get_mut(surface) {
            c.pending = None;
            c.pending_body_size = None;
        }
    }
    pub fn destroy_surface(&mut self, surface: &WlSurface) {
        self.remove_surface(surface);
        self.controllers.remove(surface);
    }
    fn configure(&self, surface: &WlSurface) {
        let (Some(c), Some(p)) = (self.controllers.get(surface), self.promoted.get(surface)) else {
            return;
        };
        c.resource.configure(
            p.serial,
            if p.moving {
                bead_layer_promote_v1::State::Moving
            } else {
                bead_layer_promote_v1::State::Promoted
            },
            p.body_origin.x,
            p.body_origin.y,
            p.local_body.size.w,
            p.local_body.size.h,
            p.output.name(),
        );
    }
}
impl Niri {
    pub fn promoted_surface_origin(&self, p: &PromotedLayer) -> Point<f64, Logical> {
        let output_origin = self
            .global_space
            .output_geometry(&p.output)
            .map(|g| g.loc.to_f64())
            .unwrap_or_default();
        let scale = smithay::utils::Scale::from(p.output.current_scale().fractional_scale());
        (p.body_origin - p.local_body.loc - output_origin)
            .to_physical_precise_round(scale)
            .to_logical(scale)
    }
    /// Follow workspace/collection ownership using public layout observations;
    /// promotion does not add a second layout surface or edit collection state.
    pub fn refresh_layer_promotions(&mut self) {
        let changes = self
            .layer_promote_manager_state
            .promoted
            .iter()
            .filter_map(|(surface, p)| {
                let workspace = self
                    .layout
                    .workspaces()
                    .find(|(_, _, ws)| ws.id().get() == p.workspace);
                let target = if p.moving {
                    Some((p.output.clone(), p.workspace))
                } else {
                    match workspace {
                        Some((Some(mon), _, ws)) => Some((mon.output().clone(), ws.id().get())),
                        Some((None, _, _)) => None, // monitorless or hidden collection
                        None => {
                            // Empty workspaces may be reclaimed because this root is
                            // not a layout tile. Retain it within its own collection.
                            self.layout
                                .outputs()
                                .find(|o| self.layout.collection_for_output(o) == p.collection)
                                .and_then(|o| {
                                    self.layout.monitor_for_output(o).map(|mon| {
                                        (o.clone(), mon.active_workspace_ref().id().get())
                                    })
                                })
                        }
                    }
                }?;
                let origin = self.global_space.output_geometry(&target.0)?.loc.to_f64();
                (target.0 != p.output || target.1 != p.workspace || origin != p.output_origin)
                    .then(|| (surface.clone(), target))
            })
            .collect::<Vec<_>>();
        for (surface, (output, workspace)) in changes {
            let old_origin = self
                .layer_promote_manager_state
                .promoted
                .get(&surface)
                .map(|p| p.output_origin)
                .unwrap_or_default();
            let new_origin = self
                .global_space
                .output_geometry(&output)
                .map(|g| g.loc.to_f64())
                .unwrap_or_default();
            let moving = self
                .layer_promote_manager_state
                .promoted
                .get(&surface)
                .is_some_and(|p| p.moving);
            let moving_origin = moving
                .then(|| {
                    self.seat.get_pointer().and_then(|pointer| {
                        let location = pointer.current_location();
                        pointer
                            .with_grab(|_, grab| {
                                grab.as_any()
                                    .downcast_ref::<LayerMoveGrab>()
                                    .filter(|grab| grab.surface() == &surface)
                                    .map(|grab| grab.body_origin(location))
                            })
                            .flatten()
                    })
                })
                .flatten();
            self.relocate_promoted_layer(&surface, output);
            if let Some(p) = self.layer_promote_manager_state.promoted.get_mut(&surface) {
                p.workspace = workspace;
                p.body_origin = moving_origin.unwrap_or(p.body_origin + new_origin - old_origin);
                p.output_origin = new_origin;
            }
            self.layer_promote_manager_state.configure(&surface);
            self.queue_redraw_all();
        }
    }
    pub fn promotion_before_arrange(&mut self, surface: &WlSurface) {
        if let Some(p) = self.layer_promote_manager_state.promoted.get_mut(surface) {
            with_states(surface, |states| {
                let mut cached = states.cached_state.get::<LayerSurfaceCachedState>();
                p.dock_zone = cached.pending().exclusive_zone;
                cached.current().exclusive_zone = ExclusiveZone::DontCare;
            });
        }
    }
    pub fn promotion_visible(&self, p: &PromotedLayer) -> bool {
        !self.is_locked()
            && !self.layout.is_overview_open()
            && self.layout.outputs().any(|output| {
                self.layout
                    .collection_switch_transform_for_workspace(output, p.workspace)
                    .is_some()
            })
            && is_mapped(p.layer.wl_surface())
    }
    pub fn promotion_workspace_input_eligible(&self, p: &PromotedLayer) -> bool {
        self.promotion_visible(p)
            && !self
                .layout
                .collection_switch_involves_workspace(&p.output, p.workspace)
            && self
                .layout
                .monitor_for_output(&p.output)
                .is_some_and(|mon| mon.active_workspace_ref().id().get() == p.workspace)
    }
    pub fn promotion_can_receive_input(&self, p: &PromotedLayer) -> bool {
        self.promotion_workspace_input_eligible(p)
            && !self.screenshot_ui.is_open()
            && !self.exit_confirm_dialog.is_open()
            && !self.window_mru_ui.is_open()
    }
    pub fn promoted_layers_for_output(&self, output: &Output) -> Vec<&PromotedLayer> {
        let mut layers = self
            .layer_promote_manager_state
            .promoted
            .values()
            .filter(|p| {
                self.layout
                    .collection_switch_transform_for_workspace(output, p.workspace)
                    .is_some()
                    && self.promotion_visible(p)
            })
            .collect::<Vec<_>>();
        layers.sort_by_key(|p| std::cmp::Reverse(p.order));
        layers
    }
    pub fn layer_promotion_can_receive_input(&self, surface: &WlSurface) -> bool {
        self.layer_promote_manager_state
            .promoted
            .get(surface)
            .is_none_or(|p| self.promotion_workspace_input_eligible(p))
    }
    pub fn finish_layer_move(&mut self, surface: &WlSurface) {
        if let Some(p) = self.layer_promote_manager_state.promoted.get_mut(surface) {
            p.moving = false;
            self.layer_promote_manager_state.configure(surface);
            self.queue_redraw_all();
        }
    }
    pub fn promotion_output_removed(&mut self, output: &Output) {
        let target = self.layout.outputs().find(|o| *o != output).cloned();
        let surfaces = self
            .layer_promote_manager_state
            .promoted
            .iter()
            .filter(|(_, p)| &p.output == output)
            .map(|(s, _)| s.clone())
            .collect::<Vec<_>>();
        for surface in surfaces {
            if let Some(target) = &target {
                self.relocate_promoted_layer(&surface, target.clone());
                if let Some(p) = self.layer_promote_manager_state.promoted.get_mut(&surface) {
                    if let Some(geo) = self.global_space.output_geometry(target) {
                        p.body_origin = geo.loc.to_f64() + Point::from((16., 16.));
                    }
                    p.moving = false;
                }
                self.layer_promote_manager_state.configure(&surface);
            } else {
                if let Some(p) = self.layer_promote_manager_state.promoted.get(&surface) {
                    let layer = p.layer.clone();
                    layer.layer_surface().send_close();
                    layer_map_for_output(output).unmap_layer(&layer);
                    self.mapped_layer_surfaces.remove(&layer);
                }
                self.layer_promote_manager_state.remove_surface(&surface);
            }
        }
    }
    fn relocate_promoted_layer(&mut self, surface: &WlSurface, output: Output) {
        let Some(p) = self.layer_promote_manager_state.promoted.get_mut(surface) else {
            return;
        };
        if p.output == output {
            return;
        }
        let old = p.output.clone();
        let layer = p.layer.clone();
        layer_map_for_output(&old).unmap_layer(&layer);
        layer_map_for_output(&output)
            .map_layer(&layer)
            .expect("unmapped promotion layer");
        p.output = output.clone();
        p.output_origin = self
            .global_space
            .output_geometry(&output)
            .map(|g| g.loc.to_f64())
            .unwrap_or_default();
        if let Some(mon) = self.layout.monitor_for_output(&output) {
            p.workspace = mon.active_workspace_ref().id().get();
        }
        p.collection = self.layout.collection_for_output(&output);
        let scale = output.current_scale();
        with_states(surface, |states| {
            send_scale_transform(surface, states, scale, output.current_transform())
        });
        if let Some(mapped) = self.mapped_layer_surfaces.get_mut(&layer) {
            mapped.update_sizes(output_size(&output), scale.fractional_scale());
        }
        self.output_resized(&old);
        self.output_resized(&output);
    }
    pub fn move_promoted_layer(
        &mut self,
        surface: &WlSurface,
        origin: Point<f64, Logical>,
        pointer: Point<f64, Logical>,
    ) {
        if let Some((output, _)) = self.output_under(pointer) {
            let output = output.clone();
            self.relocate_promoted_layer(surface, output);
        }
        if let Some(p) = self.layer_promote_manager_state.promoted.get_mut(surface) {
            p.body_origin = origin;
            self.layer_promote_manager_state.configure(surface);
            self.queue_redraw_all();
        }
    }
    fn dock_promoted_layer(&mut self, surface: &WlSurface, serial: u32, cancel: bool) {
        if cancel {
            if let Some(p) = self.layer_promote_manager_state.promoted.get(surface) {
                let origin = p.origin_output.clone();
                if self.output_state.contains_key(&origin) {
                    self.relocate_promoted_layer(surface, origin);
                }
            }
        }
        let Some(p) = self.layer_promote_manager_state.promoted.remove(surface) else {
            self.layer_promote_manager_state
                .reject(surface, serial, "surface is already docked");
            return;
        };
        set_exclusive_zone(surface, p.dock_zone);
        self.output_resized(&p.output);
        let map = layer_map_for_output(&p.output);
        let origin = self
            .global_space
            .output_geometry(&p.output)
            .map(|g| g.loc.to_f64())
            .unwrap_or_default()
            + map
                .layer_geometry(&p.layer)
                .map(|g| g.loc.to_f64())
                .unwrap_or_default()
            + p.local_body.loc;
        let start = self
            .output_state
            .get(&p.output)
            .map(|s| s.frame_clock.next_presentation_time())
            .unwrap_or_else(|| self.clock.now_unadjusted());
        if let Some(mapped) = self.mapped_layer_surfaces.get_mut(&p.layer) {
            mapped.start_promotion_return(serial, p.body_origin - origin, start);
        }
        if let Some(c) = self.layer_promote_manager_state.controllers.get(surface) {
            c.resource.configure(
                serial,
                bead_layer_promote_v1::State::Docked,
                origin.x,
                origin.y,
                p.local_body.size.w,
                p.local_body.size.h,
                p.output.name(),
            );
        }
        self.queue_redraw_all();
    }
}
impl State {
    fn promotion_layer(&self, surface: &WlSurface) -> Option<(Output, LayerSurface)> {
        self.niri.layout.outputs().find_map(|output| {
            let map = layer_map_for_output(output);
            map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .map(|layer| (output.clone(), layer.clone()))
        })
    }
    fn promotion_grab(
        &self,
        surface: &WlSurface,
        seat: &WlSeat,
        serial: u32,
    ) -> Option<GrabStartData<State>> {
        if Seat::<State>::from_resource(seat).as_ref() != Some(&self.niri.seat) {
            return None;
        }
        let pointer = self.niri.seat.get_pointer()?;
        let mut start = None;
        pointer.with_grab(|grab_serial, grab| {
            if grab_serial == Serial::from(serial) && !Self::is_dnd_grab(grab.as_any()) {
                let data = grab.start_data();
                if data.focus.as_ref().is_some_and(|(focus, _)| {
                    let mut root = focus.clone();
                    while let Some(parent) = get_parent(&root) {
                        root = parent;
                    }
                    root == *surface
                }) {
                    start = Some(data.clone());
                }
            }
        });
        start
    }
    fn begin_layer_move(&mut self, surface: &WlSurface, seat: &WlSeat, serial: u32) -> bool {
        if !self
            .niri
            .layer_promote_manager_state
            .promoted
            .get(surface)
            .is_some_and(|p| self.niri.promotion_can_receive_input(p))
        {
            return false;
        }
        let Some(start) = self.promotion_grab(surface, seat, serial) else {
            return false;
        };
        let Some(p) = self
            .niri
            .layer_promote_manager_state
            .promoted
            .get_mut(surface)
        else {
            return false;
        };
        if p.moving {
            return false;
        }
        p.moving = true;
        let origin = p.body_origin;
        let pointer = self.niri.seat.get_pointer().unwrap();
        let location = pointer.current_location();
        let grab = LayerMoveGrab::new(start, surface.clone(), origin);
        self.niri
            .move_promoted_layer(surface, grab.body_origin(location), location);
        pointer.set_grab(self, grab, Serial::from(serial), Focus::Keep);
        self.niri.layer_shell_on_demand_focus = self
            .niri
            .layer_promote_manager_state
            .promoted
            .get(surface)
            .map(|p| p.layer.clone());
        true
    }
    /// Root commit is the only switch between ordinary layer and floating ownership.
    pub fn layer_promotion_commit(
        &mut self,
        surface: &WlSurface,
        previous_origin: Option<Point<f64, Logical>>,
    ) {
        if !is_mapped(surface) {
            self.niri
                .layer_promote_manager_state
                .remove_surface(surface);
            return;
        }
        let (pending, body_size) = self
            .niri
            .layer_promote_manager_state
            .controllers
            .get_mut(surface)
            .map(|c| (c.pending.take(), c.pending_body_size.take()))
            .unwrap_or_default();
        if pending.is_none() && body_size.is_none() {
            return;
        }
        if let Some(pending) = pending {
            match pending {
                Pending::Promote {
                    serial,
                    seat,
                    button_serial,
                    body,
                } => {
                    if self.niri.is_locked() {
                        self.niri.layer_promote_manager_state.reject(
                            surface,
                            serial,
                            "session is locked",
                        );
                        return;
                    }
                    let Some((output, layer)) = self.promotion_layer(surface) else {
                        self.niri.layer_promote_manager_state.reject(
                            surface,
                            serial,
                            "surface has no output",
                        );
                        return;
                    };
                    let valid =
                        with_states(surface, crate::utils::surface_geo).is_some_and(|geo| {
                            body.loc.x >= 0.
                                && body.loc.y >= 0.
                                && body.loc.x + body.size.w <= geo.size.w as f64
                                && body.loc.y + body.size.h <= geo.size.h as f64
                        });
                    if !valid {
                        self.niri.layer_promote_manager_state.reject(
                            surface,
                            serial,
                            "body is outside the current surface",
                        );
                        return;
                    }
                    if seat
                        .as_ref()
                        .is_some_and(|s| self.promotion_grab(surface, s, button_serial).is_none())
                    {
                        self.niri.layer_promote_manager_state.reject(
                            surface,
                            serial,
                            "pointer grab is no longer valid",
                        );
                        return;
                    }
                    let Some(workspace) = self
                        .niri
                        .layout
                        .monitor_for_output(&output)
                        .map(|mon| mon.active_workspace_ref().id().get())
                    else {
                        self.niri.layer_promote_manager_state.reject(
                            surface,
                            serial,
                            "output has no visible workspace",
                        );
                        return;
                    };
                    let output_origin = self
                        .niri
                        .global_space
                        .output_geometry(&output)
                        .unwrap()
                        .loc
                        .to_f64();
                    let root_origin = previous_origin.unwrap_or_else(|| {
                        output_origin
                            + layer_map_for_output(&output)
                                .layer_geometry(&layer)
                                .unwrap()
                                .loc
                                .to_f64()
                            + self
                                .niri
                                .mapped_layer_surfaces
                                .get(&layer)
                                .map(|m| m.bob_offset())
                                .unwrap_or_default()
                    });
                    let origin = root_origin + body.loc;
                    if let Some(mapped) = self.niri.mapped_layer_surfaces.get_mut(&layer) {
                        mapped.clear_promotion_return();
                    }
                    let manager = &mut self.niri.layer_promote_manager_state;
                    if manager.is_promoted(surface) {
                        manager.reject(surface, serial, "surface is already promoted");
                        return;
                    }
                    manager.order = manager.order.wrapping_add(1);
                    let dock_zone = layer.cached_state().exclusive_zone;
                    let collection = self.niri.layout.collection_for_output(&output);
                    manager.promoted.insert(
                        surface.clone(),
                        PromotedLayer {
                            layer: layer.clone(),
                            output: output.clone(),
                            origin_output: output,
                            local_body: body,
                            body_origin: origin,
                            output_origin,
                            workspace,
                            collection,
                            moving: false,
                            serial,
                            order: manager.order,
                            dock_zone,
                        },
                    );
                    set_exclusive_zone(surface, ExclusiveZone::DontCare);
                    if let Some(seat) = seat {
                        self.begin_layer_move(surface, &seat, button_serial);
                    }
                    self.niri.layer_shell_on_demand_focus = Some(layer);
                    self.niri.layer_promote_manager_state.configure(surface);
                    self.niri.queue_redraw_all();
                }
                Pending::Dock(serial) => self.niri.dock_promoted_layer(surface, serial, false),
                Pending::Cancel(serial) => self.niri.dock_promoted_layer(surface, serial, true),
            }
        }
        if let Some(size) = body_size {
            let manager = &mut self.niri.layer_promote_manager_state;
            let Some(p) = manager.promoted.get_mut(surface) else {
                return;
            };
            let fits = with_states(surface, crate::utils::surface_geo).is_some_and(|geo| {
                p.local_body.loc.x + size.w <= geo.size.w as f64
                    && p.local_body.loc.y + size.h <= geo.size.h as f64
            });
            if !fits {
                let serial = p.serial;
                manager.reject(
                    surface,
                    serial,
                    "body size is outside the committed root texture",
                );
                return;
            }
            p.local_body.size = size;
            manager.configure(surface);
            self.niri.queue_redraw_all();
        }
    }
}
impl GlobalDispatch<BeadLayerPromoteManagerV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<BeadLayerPromoteManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}
impl Dispatch<BeadLayerPromoteManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &BeadLayerPromoteManagerV1,
        request: bead_layer_promote_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let bead_layer_promote_manager_v1::Request::GetPromotion { id, surface } = request {
            if state
                .niri
                .layer_promote_manager_state
                .controllers
                .contains_key(&surface)
            {
                resource.post_error(1u32, "duplicate promotion controller");
                return;
            }
            if !state
                .niri
                .layer_shell_state
                .layer_surfaces()
                .any(|layer| layer.wl_surface() == &surface && layer.alive())
            {
                resource.post_error(0u32, "surface must have a live layer-shell role");
                return;
            }
            let controller = init.init(id, PromotionData(surface.clone()));
            state.niri.layer_promote_manager_state.controllers.insert(
                surface,
                Controller {
                    resource: controller,
                    pending: None,
                    last_serial: None,
                    pending_body_size: None,
                },
            );
        }
    }
}
impl Dispatch<BeadLayerPromoteV1, PromotionData> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &BeadLayerPromoteV1,
        request: bead_layer_promote_v1::Request,
        data: &PromotionData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if !state
            .niri
            .layer_promote_manager_state
            .controllers
            .get(&data.0)
            .is_some_and(|controller| &controller.resource == resource)
        {
            return;
        }
        if let bead_layer_promote_v1::Request::SetBodySize { width, height } = request {
            if !width.is_finite()
                || !height.is_finite()
                || width <= 0.
                || height <= 0.
                || width > 32768.
                || height > 32768.
            {
                resource.post_error(0u32, "invalid body size");
                return;
            }
            if let Some(controller) = state
                .niri
                .layer_promote_manager_state
                .controllers
                .get_mut(&data.0)
            {
                controller.pending_body_size = Some(Size::from((width, height)));
            }
            return;
        }
        if let bead_layer_promote_v1::Request::Move {
            seat,
            button_serial,
        } = request
        {
            if !state.begin_layer_move(&data.0, &seat, button_serial) {
                let serial = state
                    .niri
                    .layer_promote_manager_state
                    .controllers
                    .get(&data.0)
                    .and_then(|c| c.last_serial)
                    .unwrap_or(0);
                resource.rejected(serial, "invalid pointer grab or docked surface".to_owned());
            }
            return;
        }
        if matches!(request, bead_layer_promote_v1::Request::Destroy) {
            state
                .niri
                .layer_promote_manager_state
                .controllers
                .remove(&data.0);
            if state.niri.layer_promote_manager_state.is_promoted(&data.0) {
                state.niri.dock_promoted_layer(&data.0, 0, true);
            }
            return;
        }
        let (serial, pending) = match request {
            bead_layer_promote_v1::Request::Promote {
                serial,
                seat,
                button_serial,
                x,
                y,
                width,
                height,
            } => {
                if ![x, y, width, height].iter().all(|v| v.is_finite())
                    || width <= 0.
                    || height <= 0.
                    || width > 32768.
                    || height > 32768.
                    || (seat.is_none() && button_serial != 0)
                {
                    resource.post_error(0u32, "invalid body geometry or command grab");
                    return;
                }
                (
                    serial,
                    Pending::Promote {
                        serial,
                        seat,
                        button_serial,
                        body: Rectangle::new(Point::from((x, y)), Size::from((width, height))),
                    },
                )
            }
            bead_layer_promote_v1::Request::Dock { serial } => (serial, Pending::Dock(serial)),
            bead_layer_promote_v1::Request::Cancel { serial } => (serial, Pending::Cancel(serial)),
            _ => return,
        };
        if let Some(c) = state
            .niri
            .layer_promote_manager_state
            .controllers
            .get_mut(&data.0)
        {
            if c.last_serial
                .is_some_and(|last| serial.wrapping_sub(last) as i32 <= 0)
            {
                return;
            }
            c.last_serial = Some(serial);
            c.pending = Some(pending);
        }
    }
    fn destroyed(
        state: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        resource: &BeadLayerPromoteV1,
        data: &PromotionData,
    ) {
        let manager = &mut state.niri.layer_promote_manager_state;
        if manager
            .controllers
            .get(&data.0)
            .is_some_and(|c| &c.resource == resource)
        {
            manager.remove_surface(&data.0);
            manager.controllers.remove(&data.0);
            state.niri.queue_redraw_all();
        }
    }
}
