use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::backend::renderer::element::Id;

use crate::layer::motion::{
    LayerMotionGroup, MotionCurve, MotionPose, OpacityMode,
};
use crate::protocols::raw::bead_layer_motion::v1::server::{
    bead_layer_motion_manager_v1, bead_layer_motion_v1,
};
use bead_layer_motion_manager_v1::BeadLayerMotionManagerV1;
use bead_layer_motion_v1::BeadLayerMotionV1;

const VERSION: u32 = 1;
const MAX_PLANES: usize = 64;

#[derive(Clone, Debug)]
pub struct MotionPlane {
    pub id: u32,
    pub render_id: Id,
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
    last_serial: Option<u32>,
}

pub struct LayerMotionManagerState {
    controllers: HashMap<WlSurface, Controller>,
}

pub trait LayerMotionHandler {
    fn layer_motion_manager_state(&mut self) -> &mut LayerMotionManagerState;
    fn layer_motion_now(&self) -> Duration;
    fn layer_motion_attach(&mut self, surface: &WlSurface) -> Option<Arc<Mutex<LayerMotionRuntime>>>;
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
        controller.committed_planes.clone_from(&controller.pending_planes);
        controller.committed_curve = controller.pending_curve;
        let mut runtime = controller.runtime.lock().unwrap();
        runtime.planes.clone_from(&controller.committed_planes);

        if let Some(serial) = controller.pending_cancel.take() {
            if runtime.active.as_ref().is_some_and(|motion| motion.serial == serial) {
                let current = runtime
                    .active
                    .as_ref()
                    .unwrap()
                    .sample(next_presentation);
                runtime.active = None;
                runtime.current.clone_from(&current);
                send_final_state(&controller.resource, &current);
                controller.resource.cancelled(serial);
                controller.last_serial = None;
            }
        }

        let Some((serial, duration, retarget)) = controller.pending_animate.take() else {
            return;
        };
        if duration.is_zero() || duration > Duration::from_secs(10) {
            controller.resource.post_error(
                bead_layer_motion_v1::Error::InvalidDuration,
                "duration must be between 1 and 10000 ms",
            );
            return;
        }
        if let Some(old) = runtime.active.take() {
            let current = old.sample(next_presentation);
            send_final_state(&controller.resource, &current);
            controller.resource.cancelled(old.serial);
            runtime.current.clone_from(&current);
        }

        let from = if retarget {
            runtime.current.clone()
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
        if let Some(motion) = LayerMotionGroup::new(
            serial,
            from,
            to,
            modes,
            next_presentation,
            duration,
            controller.committed_curve,
        ) {
            runtime.current = motion.sample(next_presentation);
            runtime.active = Some(motion);
            controller.last_serial = Some(serial);
        }
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
            runtime.active = None;
            send_final_state(&controller.resource, &final_poses);
            controller.resource.done(serial);
            controller.last_serial = None;
            false
        } else {
            runtime.current = motion.sample(presentation_time);
            true
        }
    }

    pub fn remove_surface(&mut self, surface: &WlSurface, presentation_time: Duration) {
        let Some(mut controller) = self.controllers.remove(surface) else {
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
        let Some(motion) = runtime.active.take() else {
            return;
        };
        let current = motion.sample(presentation_time);
        runtime.current.clone_from(&current);
        send_final_state(&controller.resource, &current);
        controller.resource.cancelled(motion.serial);
        controller.last_serial = None;
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
                        last_serial: None,
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
        if let bead_layer_motion_v1::Request::Destroy = request {
            let now = state.layer_motion_now();
            state.layer_motion_manager_state().remove_surface(&data.surface, now);
            return;
        }
        let manager = state.layer_motion_manager_state();
        let Some(controller) = manager.controllers.get_mut(&data.surface) else {
            return;
        };
        if controller.resource != *resource {
            return;
        }
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
                if controller.pending_planes.iter().any(|item| item.id == plane) {
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
                    source,
                    from: pose,
                    to: pose,
                    opacity_mode: OpacityMode::Linear,
                });
            }
            bead_layer_motion_v1::Request::SetPlaneTo { plane, x, y, width, height, clip_x, clip_y, clip_width, clip_height, opacity } => {
                let Some(item) = controller.pending_planes.iter_mut().find(|item| item.id == plane) else {
                    resource.post_error(bead_layer_motion_v1::Error::UnknownPlane, "unknown plane identifier");
                    return;
                };
                let Some(pose) = MotionPose::new(x, y, width, height, clip_x, clip_y, clip_width, clip_height, opacity) else {
                    resource.post_error(bead_layer_motion_v1::Error::InvalidGeometry, "invalid plane geometry");
                    return;
                };
                item.to = pose;
            }
            bead_layer_motion_v1::Request::SetPlaneFrom { plane, x, y, width, height, clip_x, clip_y, clip_width, clip_height, opacity } => {
                let Some(item) = controller.pending_planes.iter_mut().find(|item| item.id == plane) else {
                    resource.post_error(bead_layer_motion_v1::Error::UnknownPlane, "unknown plane identifier");
                    return;
                };
                let Some(pose) = MotionPose::new(x, y, width, height, clip_x, clip_y, clip_width, clip_height, opacity) else {
                    resource.post_error(bead_layer_motion_v1::Error::InvalidGeometry, "invalid plane geometry");
                    return;
                };
                item.from = pose;
            }
            bead_layer_motion_v1::Request::SetPlaneOpacityMode { plane, mode, threshold_millionths } => {
                let Some(item) = controller.pending_planes.iter_mut().find(|item| item.id == plane) else {
                    resource.post_error(bead_layer_motion_v1::Error::UnknownPlane, "unknown plane identifier");
                    return;
                };
                let opacity_mode = match mode {
                        WEnum::Value(bead_layer_motion_v1::OpacityMode::Linear) => Some(OpacityMode::Linear),
                        WEnum::Value(bead_layer_motion_v1::OpacityMode::OpaqueAbove) if threshold_millionths <= 1_000_000 => Some(OpacityMode::OpaqueAbove(threshold_millionths)),
                        _ => None,
                    };
                let Some(opacity_mode) = opacity_mode else {
                    resource.post_error(bead_layer_motion_v1::Error::InvalidGeometry, "invalid opacity mode or threshold");
                    return;
                };
                item.opacity_mode = opacity_mode;
            }
            bead_layer_motion_v1::Request::SetCurve { curve } => {
                controller.pending_curve = match curve {
                    WEnum::Value(bead_layer_motion_v1::Curve::Linear) => MotionCurve::Linear,
                    WEnum::Value(bead_layer_motion_v1::Curve::EaseOutCubic) => MotionCurve::EaseOutCubic,
                    WEnum::Value(bead_layer_motion_v1::Curve::EaseInOutCubic) => MotionCurve::EaseInOutCubic,
                    WEnum::Value(bead_layer_motion_v1::Curve::EmphasizedAccelerate) => MotionCurve::EmphasizedAccelerate,
                    WEnum::Value(bead_layer_motion_v1::Curve::Spring) => MotionCurve::CriticalSpring,
                    WEnum::Unknown(_) => {
                        resource.post_error(bead_layer_motion_v1::Error::InvalidGeometry, "unknown curve");
                        return;
                    }
                };
            }
            bead_layer_motion_v1::Request::Animate { serial, duration_ms, retarget } => {
                if duration_ms == 0 || duration_ms > 10_000 {
                    resource.post_error(bead_layer_motion_v1::Error::InvalidDuration, "duration must be between 1 and 10000 ms");
                    return;
                }
                controller.pending_animate = Some((serial, Duration::from_millis(duration_ms as u64), retarget != 0));
            }
            bead_layer_motion_v1::Request::Cancel { serial } => {
                controller.pending_cancel = Some(serial);
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
