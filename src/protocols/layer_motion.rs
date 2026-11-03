use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bead_layer_motion_manager_v1::BeadLayerMotionManagerV1;
use bead_layer_motion_v1::BeadLayerMotionV1;
use smithay::backend::renderer::element::Id;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::wayland::compositor::with_states;

use crate::layer::motion::{LayerMotionGroup, MotionCurve, MotionPose, OpacityMode};
use crate::protocols::raw::bead_layer_motion::v1::server::{
    bead_layer_motion_manager_v1, bead_layer_motion_v1,
};
use crate::render_helpers::damage::ExtraDamage;

const VERSION: u32 = 1;
const MAX_PLANES: usize = 64;

#[derive(Clone, Debug)]
pub struct MotionPlane {
    pub id: u32,
    pub render_id: Id,
    pub damage: ExtraDamage,
    pub source: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
    pub from: MotionPose,
    pub to: MotionPose,
    pub opacity_mode: OpacityMode,
}

/// State shared by the protocol controller and its mapped layer surface.
#[derive(Debug, Default)]
pub struct LayerMotionRuntime {
    pub planes: Vec<MotionPlane>,
    pub current: Vec<(u32, MotionPose)>,
    pub active: Option<LayerMotionGroup>,
}

impl LayerMotionRuntime {
    fn cancel_at(&mut self, presentation_time: Duration) -> Option<(u32, Vec<(u32, MotionPose)>)> {
        let motion = self.active.take()?;
        let serial = motion.serial;
        let current = motion.sample(presentation_time);
        self.current.clone_from(&current);
        Some((serial, current))
    }
}

#[derive(Clone)]
pub struct ControllerData {
    surface: WlSurface,
}

struct Controller {
    resource: BeadLayerMotionV1,
    runtime: Arc<Mutex<LayerMotionRuntime>>,
    pending_planes: Vec<MotionPlane>,
    committed_planes: Vec<MotionPlane>,
    pending_curve: MotionCurve,
    committed_curve: MotionCurve,
    pending_animate: Option<(u32, Duration, bool)>,
    pending_cancel: Option<u32>,
    pending_reset: bool,
    last_seen_serial: Option<u32>,
}

pub struct LayerMotionManagerState {
    controllers: HashMap<WlSurface, Controller>,
}

pub trait LayerMotionHandler {
    fn layer_motion_manager_state(&mut self) -> &mut LayerMotionManagerState;
    fn layer_motion_now(&self) -> Duration;
    fn layer_motion_attach(
        &mut self,
        surface: &WlSurface,
    ) -> Option<Arc<Mutex<LayerMotionRuntime>>>;
}

impl LayerMotionManagerState {
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<BeadLayerMotionManagerV1, ()>,
        D: Dispatch<BeadLayerMotionManagerV1, ()>,
        D: Dispatch<BeadLayerMotionV1, ControllerData>,
        D: LayerMotionHandler + 'static,
    {
        display.create_global::<D, BeadLayerMotionManagerV1, _>(VERSION, ());
        Self {
            controllers: HashMap::new(),
        }
    }

    pub fn runtime(&self, surface: &WlSurface) -> Option<Arc<Mutex<LayerMotionRuntime>>> {
        self.controllers
            .get(surface)
            .map(|controller| controller.runtime.clone())
    }

    /// Apply the double-buffered protocol state at the root surface commit.
    pub fn commit(&mut self, surface: &WlSurface, next_presentation: Duration) {
        let Some(controller) = self.controllers.get_mut(surface) else {
            return;
        };
        let surface_size =
            with_states(surface, crate::utils::surface_geo).map(|geometry| geometry.size.to_f64());
        let Some(surface_size) = surface_size else {
            controller.resource.post_error(
                bead_layer_motion_v1::Error::InvalidGeometry,
                "motion planes require a mapped surface buffer",
            );
            return;
        };
        if controller.pending_planes.iter().any(|plane| {
            plane.source.loc.x + plane.source.size.w > surface_size.w
                || plane.source.loc.y + plane.source.size.h > surface_size.h
        }) {
            controller.resource.post_error(
                bead_layer_motion_v1::Error::InvalidGeometry,
                "plane source rectangle extends beyond the surface texture",
            );
            return;
        }
        let mut runtime = controller.runtime.lock().unwrap();
        if std::mem::take(&mut controller.pending_reset) {
            if let Some((serial, current)) = runtime.cancel_at(next_presentation) {
                send_final_state(&controller.resource, &current);
                controller.resource.cancelled(serial);
            }
            runtime.planes.clear();
            runtime.current.clear();
            controller.committed_planes.clear();
        }
        controller
            .committed_planes
            .clone_from(&controller.pending_planes);
        controller.committed_curve = controller.pending_curve;
        for plane in &mut controller.committed_planes {
            if let Some(previous) = runtime.planes.iter().find(|old| old.id == plane.id) {
                plane.damage = previous.damage.clone();
            }
            plane.damage.damage_all();
        }
        runtime.planes.clone_from(&controller.committed_planes);

        if let Some(serial) = controller.pending_cancel.take() {
            if runtime
                .active
                .as_ref()
                .is_some_and(|motion| motion.serial == serial)
            {
                let current = runtime.active.as_ref().unwrap().sample(next_presentation);
                runtime.active = None;
                runtime.current.clone_from(&current);
                send_final_state(&controller.resource, &current);
                controller.resource.cancelled(serial);
            }
        }

        if runtime.active.is_none() {
            let current = runtime.current.clone();
            runtime.current = controller
                .committed_planes
                .iter()
                .map(|plane| {
                    let pose = current
                        .iter()
                        .find(|(id, _)| *id == plane.id)
                        .map_or(plane.from, |(_, pose)| *pose);
                    (plane.id, pose)
                })
                .collect();
        }

        let Some((serial, duration, retarget)) = controller.pending_animate.take() else {
            return;
        };
        if controller
            .last_seen_serial
            .is_some_and(|last| !serial_is_newer(serial, last))
        {
            return;
        }
        if duration.is_zero() || duration > Duration::from_secs(10) {
            controller.resource.post_error(
                bead_layer_motion_v1::Error::InvalidDuration,
                "duration must be between 1 and 10000 ms",
            );
            return;
        }
        let current = runtime
            .active
            .as_ref()
            .map(|old| old.sample(next_presentation));
        let from = if retarget {
            current.clone().unwrap_or_else(|| {
                if runtime.current.is_empty() {
                    controller
                        .committed_planes
                        .iter()
                        .map(|plane| (plane.id, plane.from))
                        .collect()
                } else {
                    runtime.current.clone()
                }
            })
        } else {
            controller
                .committed_planes
                .iter()
                .map(|plane| (plane.id, plane.from))
                .collect()
        };
        let to = controller
            .committed_planes
            .iter()
            .map(|plane| (plane.id, plane.to))
            .collect();
        let modes = controller
            .committed_planes
            .iter()
            .map(|plane| (plane.id, plane.opacity_mode))
            .collect();
        let Some(motion) = LayerMotionGroup::new(
            serial,
            from,
            to,
            modes,
            next_presentation,
            duration,
            controller.committed_curve,
        ) else {
            controller.resource.post_error(
                bead_layer_motion_v1::Error::InvalidGeometry,
                "plane states do not form a valid animation group",
            );
            return;
        };
        if let Some(old) = runtime.active.take() {
            let old_current = current.unwrap_or_else(|| old.sample(next_presentation));
            send_final_state(&controller.resource, &old_current);
            controller.resource.cancelled(old.serial);
        }
        runtime.current = motion.sample(next_presentation);
        for plane in &mut runtime.planes {
            plane.damage.damage_all();
        }
        runtime.active = Some(motion);
        controller.last_seen_serial = Some(serial);
    }

    /// Advance a controller for one output's predicted presentation timestamp.
    pub fn tick(&mut self, surface: &WlSurface, presentation_time: Duration) -> bool {
        let Some(controller) = self.controllers.get_mut(surface) else {
            return false;
        };
        let mut runtime = controller.runtime.lock().unwrap();
        let Some(motion) = runtime.active.as_ref() else {
            return false;
        };
        if motion.is_complete(presentation_time) {
            let final_poses = motion.final_poses();
            let serial = motion.serial;
            runtime.current.clone_from(&final_poses);
            for plane in &mut runtime.planes {
                plane.damage.damage_all();
            }
            runtime.active = None;
            send_final_state(&controller.resource, &final_poses);
            controller.resource.done(serial);
            false
        } else {
            runtime.current = motion.sample(presentation_time);
            for plane in &mut runtime.planes {
                plane.damage.damage_all();
            }
            true
        }
    }

    pub fn remove_surface(&mut self, surface: &WlSurface, presentation_time: Duration) {
        let Some(controller) = self.controllers.remove(surface) else {
            return;
        };
        let mut runtime = controller.runtime.lock().unwrap();
        if let Some(motion) = &runtime.active {
            let current = motion.sample(presentation_time);
            send_final_state(&controller.resource, &current);
            controller.resource.cancelled(motion.serial);
        }
        runtime.active = None;
        runtime.current.clear();
        runtime.planes.clear();
    }

    /// Stop the current timeline while keeping the controller reusable after remap.
    pub fn unmap_surface(&mut self, surface: &WlSurface, presentation_time: Duration) {
        let Some(controller) = self.controllers.get_mut(surface) else {
            return;
        };
        let mut runtime = controller.runtime.lock().unwrap();
        let Some((serial, current)) = runtime.cancel_at(presentation_time) else {
            return;
        };
        send_final_state(&controller.resource, &current);
        controller.resource.cancelled(serial);
    }
}

fn send_final_state(resource: &BeadLayerMotionV1, poses: &[(u32, MotionPose)]) {
    for (plane, pose) in poses {
        resource.plane_state(
            *plane,
            pose.destination.loc.x,
            pose.destination.loc.y,
            pose.destination.size.w,
            pose.destination.size.h,
            pose.clip.loc.x,
            pose.clip.loc.y,
            pose.clip.size.w,
            pose.clip.size.h,
            pose.opacity,
        );
    }
}

fn serial_is_newer(candidate: u32, previous: u32) -> bool {
    let distance = candidate.wrapping_sub(previous);
    distance != 0 && distance < (1 << 31)
}

impl<D> GlobalDispatch<BeadLayerMotionManagerV1, (), D> for LayerMotionManagerState
where
    D: GlobalDispatch<BeadLayerMotionManagerV1, ()>,
    D: Dispatch<BeadLayerMotionManagerV1, ()>,
    D: Dispatch<BeadLayerMotionV1, ControllerData>,
    D: LayerMotionHandler + 'static,
{
    fn bind(
        _state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        manager: New<BeadLayerMotionManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(manager, ());
    }
}

impl<D> Dispatch<BeadLayerMotionManagerV1, (), D> for LayerMotionManagerState
where
    D: Dispatch<BeadLayerMotionManagerV1, ()>,
    D: Dispatch<BeadLayerMotionV1, ControllerData>,
    D: LayerMotionHandler + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &BeadLayerMotionManagerV1,
        request: bead_layer_motion_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            bead_layer_motion_manager_v1::Request::Destroy => (),
            bead_layer_motion_manager_v1::Request::GetMotion { id, surface } => {
                let manager = state.layer_motion_manager_state();
                if manager.controllers.contains_key(&surface) {
                    resource.post_error(
                        bead_layer_motion_manager_v1::Error::DuplicateController,
                        "surface already has a layer-motion controller",
                    );
                    return;
                }
                let Some(runtime) = state.layer_motion_attach(&surface) else {
                    resource.post_error(
                        bead_layer_motion_manager_v1::Error::InvalidSurface,
                        "surface does not have a layer-shell role",
                    );
                    return;
                };
                let data = ControllerData {
                    surface: surface.clone(),
                };
                let resource = data_init.init(id, data);
                state.layer_motion_manager_state().controllers.insert(
                    surface.clone(),
                    Controller {
                        resource,
                        runtime,
                        pending_planes: Vec::new(),
                        committed_planes: Vec::new(),
                        pending_curve: MotionCurve::Linear,
                        committed_curve: MotionCurve::Linear,
                        pending_animate: None,
                        pending_cancel: None,
                        pending_reset: false,
                        last_seen_serial: None,
                    },
                );
            }
        }
    }
}

impl<D> Dispatch<BeadLayerMotionV1, ControllerData, D> for LayerMotionManagerState
where
    D: Dispatch<BeadLayerMotionV1, ControllerData>,
    D: LayerMotionHandler + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &BeadLayerMotionV1,
        request: bead_layer_motion_v1::Request,
        data: &ControllerData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
        if state
            .layer_motion_manager_state()
            .controllers
            .get(&data.surface)
            .is_none_or(|controller| controller.resource != *resource)
        {
            return;
        }
        if let bead_layer_motion_v1::Request::Destroy = request {
            let now = state.layer_motion_now();
            state
                .layer_motion_manager_state()
                .remove_surface(&data.surface, now);
            return;
        }
        let manager = state.layer_motion_manager_state();
        let Some(controller) = manager.controllers.get_mut(&data.surface) else {
            return;
        };
        match request {
            bead_layer_motion_v1::Request::Destroy => unreachable!(),
            bead_layer_motion_v1::Request::AddPlane {
                plane,
                source_x,
                source_y,
                source_width,
                source_height,
                from_x,
                from_y,
                from_width,
                from_height,
                clip_x,
                clip_y,
                clip_width,
                clip_height,
                from_opacity,
            } => {
                if controller
                    .pending_planes
                    .iter()
                    .any(|item| item.id == plane)
                {
                    resource.post_error(
                        bead_layer_motion_v1::Error::DuplicatePlane,
                        "plane identifier already exists",
                    );
                    return;
                }
                if controller.pending_planes.len() >= MAX_PLANES {
                    resource.post_error(
                        bead_layer_motion_v1::Error::TooManyPlanes,
                        "controller plane limit exceeded",
                    );
                    return;
                }
                let pose = MotionPose::new(
                    from_x,
                    from_y,
                    from_width,
                    from_height,
                    clip_x,
                    clip_y,
                    clip_width,
                    clip_height,
                    from_opacity,
                );
                let Some(pose) = pose else {
                    resource.post_error(
                        bead_layer_motion_v1::Error::InvalidGeometry,
                        "invalid plane geometry",
                    );
                    return;
                };
                let source = smithay::utils::Rectangle::new(
                    smithay::utils::Point::from((source_x, source_y)),
                    smithay::utils::Size::from((source_width, source_height)),
                );
                if !source.loc.x.is_finite()
                    || !source.loc.y.is_finite()
                    || !source.size.w.is_finite()
                    || !source.size.h.is_finite()
                    || source.loc.x < 0.
                    || source.loc.y < 0.
                    || source.loc.x > 32_768.
                    || source.loc.y > 32_768.
                    || source.size.w <= 0.
                    || source.size.h <= 0.
                    || source.size.w > 32_768.
                    || source.size.h > 32_768.
                {
                    resource.post_error(
                        bead_layer_motion_v1::Error::InvalidGeometry,
                        "invalid source rectangle",
                    );
                    return;
                }
                controller.pending_planes.push(MotionPlane {
                    id: plane,
                    render_id: Id::new(),
                    damage: ExtraDamage::new(),
                    source,
                    from: pose,
                    to: pose,
                    opacity_mode: OpacityMode::Linear,
                });
            }
            bead_layer_motion_v1::Request::SetPlaneTo {
                plane,
                x,
                y,
                width,
                height,
                clip_x,
                clip_y,
                clip_width,
                clip_height,
                opacity,
            } => {
                let Some(item) = controller
                    .pending_planes
                    .iter_mut()
                    .find(|item| item.id == plane)
                else {
                    resource.post_error(
                        bead_layer_motion_v1::Error::UnknownPlane,
                        "unknown plane identifier",
                    );
                    return;
                };
                let Some(pose) = MotionPose::new(
                    x,
                    y,
                    width,
                    height,
                    clip_x,
                    clip_y,
                    clip_width,
                    clip_height,
                    opacity,
                ) else {
                    resource.post_error(
                        bead_layer_motion_v1::Error::InvalidGeometry,
                        "invalid plane geometry",
                    );
                    return;
                };
                item.to = pose;
            }
            bead_layer_motion_v1::Request::SetPlaneFrom {
                plane,
                x,
                y,
                width,
                height,
                clip_x,
                clip_y,
                clip_width,
                clip_height,
                opacity,
            } => {
                let Some(item) = controller
                    .pending_planes
                    .iter_mut()
                    .find(|item| item.id == plane)
                else {
                    resource.post_error(
                        bead_layer_motion_v1::Error::UnknownPlane,
                        "unknown plane identifier",
                    );
                    return;
                };
                let Some(pose) = MotionPose::new(
                    x,
                    y,
                    width,
                    height,
                    clip_x,
                    clip_y,
                    clip_width,
                    clip_height,
                    opacity,
                ) else {
                    resource.post_error(
                        bead_layer_motion_v1::Error::InvalidGeometry,
                        "invalid plane geometry",
                    );
                    return;
                };
                item.from = pose;
            }
            bead_layer_motion_v1::Request::SetPlaneOpacityMode {
                plane,
                mode,
                threshold_millionths,
            } => {
                let Some(item) = controller
                    .pending_planes
                    .iter_mut()
                    .find(|item| item.id == plane)
                else {
                    resource.post_error(
                        bead_layer_motion_v1::Error::UnknownPlane,
                        "unknown plane identifier",
                    );
                    return;
                };
                let opacity_mode = match mode {
                    WEnum::Value(bead_layer_motion_v1::OpacityMode::Linear) => {
                        Some(OpacityMode::Linear)
                    }
                    WEnum::Value(bead_layer_motion_v1::OpacityMode::OpaqueAbove)
                        if threshold_millionths <= 1_000_000 =>
                    {
                        Some(OpacityMode::OpaqueAbove(threshold_millionths))
                    }
                    _ => None,
                };
                let Some(opacity_mode) = opacity_mode else {
                    resource.post_error(
                        bead_layer_motion_v1::Error::InvalidGeometry,
                        "invalid opacity mode or threshold",
                    );
                    return;
                };
                item.opacity_mode = opacity_mode;
            }
            bead_layer_motion_v1::Request::SetCurve { curve } => {
                controller.pending_curve = match curve {
                    WEnum::Value(bead_layer_motion_v1::Curve::Linear) => MotionCurve::Linear,
                    WEnum::Value(bead_layer_motion_v1::Curve::EaseOutCubic) => {
                        MotionCurve::EaseOutCubic
                    }
                    WEnum::Value(bead_layer_motion_v1::Curve::EaseInOutCubic) => {
                        MotionCurve::EaseInOutCubic
                    }
                    WEnum::Value(bead_layer_motion_v1::Curve::EmphasizedAccelerate) => {
                        MotionCurve::EmphasizedAccelerate
                    }
                    WEnum::Value(bead_layer_motion_v1::Curve::Spring) => {
                        MotionCurve::CriticalSpring
                    }
                    WEnum::Unknown(_) => {
                        resource.post_error(
                            bead_layer_motion_v1::Error::InvalidGeometry,
                            "unknown curve",
                        );
                        return;
                    }
                };
            }
            bead_layer_motion_v1::Request::Animate {
                serial,
                duration_ms,
                retarget,
            } => {
                if duration_ms == 0 || duration_ms > 10_000 {
                    resource.post_error(
                        bead_layer_motion_v1::Error::InvalidDuration,
                        "duration must be between 1 and 10000 ms",
                    );
                    return;
                }
                controller.pending_animate = Some((
                    serial,
                    Duration::from_millis(duration_ms as u64),
                    retarget != 0,
                ));
            }
            bead_layer_motion_v1::Request::Cancel { serial } => {
                controller.pending_cancel = Some(serial);
            }
            bead_layer_motion_v1::Request::Reset => {
                controller.pending_planes.clear();
                controller.pending_curve = MotionCurve::Linear;
                controller.pending_animate = None;
                controller.pending_cancel = None;
                controller.pending_reset = true;
            }
        }
        let _ = resource;
    }
}

#[macro_export]
macro_rules! delegate_layer_motion {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::raw::bead_layer_motion::v1::server::bead_layer_motion_manager_v1::BeadLayerMotionManagerV1: ()
        ] => $crate::protocols::layer_motion::LayerMotionManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::raw::bead_layer_motion::v1::server::bead_layer_motion_manager_v1::BeadLayerMotionManagerV1: ()
        ] => $crate::protocols::layer_motion::LayerMotionManagerState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::protocols::raw::bead_layer_motion::v1::server::bead_layer_motion_v1::BeadLayerMotionV1: $crate::protocols::layer_motion::ControllerData
        ] => $crate::protocols::layer_motion::LayerMotionManagerState);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f64) -> MotionPose {
        MotionPose::new(x, 0., 20., 20., 0., 0., 20., 20., 1.).unwrap()
    }

    #[test]
    fn unmap_cancels_at_the_last_presented_group_state() {
        let motion = LayerMotionGroup::new(
            19,
            vec![(1, pose(-100.))],
            vec![(1, pose(0.))],
            vec![(1, OpacityMode::Linear)],
            Duration::ZERO,
            Duration::from_millis(100),
            MotionCurve::Linear,
        )
        .unwrap();
        let mut runtime = LayerMotionRuntime {
            planes: Vec::new(),
            current: motion.sample(Duration::ZERO),
            active: Some(motion),
        };
        let presented = runtime
            .active
            .as_ref()
            .unwrap()
            .sample(Duration::from_millis(40));
        let (serial, frozen) = runtime.cancel_at(Duration::from_millis(40)).unwrap();
        assert_eq!(serial, 19);
        assert_eq!(frozen, presented);
        assert_eq!(runtime.current, presented);
        assert!(runtime.active.is_none());
    }

    #[test]
    fn serial_order_accepts_wraparound_but_ignores_stale_values() {
        assert!(serial_is_newer(1, u32::MAX));
        assert!(!serial_is_newer(8, 8));
        assert!(!serial_is_newer(7, 8));
    }
}
