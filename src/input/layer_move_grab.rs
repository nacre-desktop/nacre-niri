//! Move the existing layer tree without transferring the client's pointer stream.
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
    RelativeMotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::niri::State;

pub struct LayerMoveGrab {
    start: GrabStartData<State>,
    surface: WlSurface,
    origin: Point<f64, Logical>,
}
impl LayerMoveGrab {
    pub fn new(
        start: GrabStartData<State>,
        surface: WlSurface,
        origin: Point<f64, Logical>,
    ) -> Self {
        Self {
            start,
            surface,
            origin,
        }
    }
    pub fn body_origin(&self, location: Point<f64, Logical>) -> Point<f64, Logical> {
        self.origin + (location - self.start.location)
    }
    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }
    fn active(&self, data: &State) -> bool {
        data.niri
            .layer_promote_manager_state
            .promoted
            .get(&self.surface)
            .is_some_and(|p| p.moving && data.niri.promotion_can_receive_input(p))
    }
    fn focus(&self, data: &State) -> Option<(WlSurface, Point<f64, Logical>)> {
        let p = data
            .niri
            .layer_promote_manager_state
            .promoted
            .get(&self.surface)?;
        let output_origin = data
            .niri
            .global_space
            .output_geometry(&p.output)?
            .loc
            .to_f64();
        Some((
            self.surface.clone(),
            data.niri.promoted_surface_origin(p) + output_origin,
        ))
    }
}
impl PointerGrab<State> for LayerMoveGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        if !data
            .niri
            .layer_promote_manager_state
            .promoted
            .get(&self.surface)
            .is_some_and(|p| data.niri.promotion_can_receive_input(p))
        {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        data.niri.move_promoted_layer(
            &self.surface,
            self.body_origin(event.location),
            event.location,
        );
        let focus = self.focus(data);
        handle.motion(data, focus, event);
    }
    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        if !data
            .niri
            .layer_promote_manager_state
            .promoted
            .get(&self.surface)
            .is_some_and(|p| data.niri.promotion_can_receive_input(p))
        {
            return;
        }
        let focus = self.focus(data);
        handle.relative_motion(data, focus, event);
    }
    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        if !data
            .niri
            .layer_promote_manager_state
            .promoted
            .get(&self.surface)
            .is_some_and(|p| data.niri.promotion_can_receive_input(p))
        {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }
    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        if self.active(data) {
            handle.axis(data, details);
        }
    }
    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        let valid = data
            .niri
            .layer_promote_manager_state
            .promoted
            .get(&self.surface)
            .is_some_and(|p| p.moving && data.niri.promotion_can_receive_input(p));
        if !valid {
            handle.unset_grab(
                self,
                data,
                SERIAL_COUNTER.next_serial(),
                crate::utils::get_monotonic_time().as_millis() as u32,
                true,
            );
        }
        handle.frame(data);
    }
    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        if self.active(data) {
            handle.gesture_swipe_begin(data, event);
        }
    }
    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        if self.active(data) {
            handle.gesture_swipe_update(data, event);
        }
    }
    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        if self.active(data) {
            handle.gesture_swipe_end(data, event);
        }
    }
    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        if self.active(data) {
            handle.gesture_pinch_begin(data, event);
        }
    }
    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        if self.active(data) {
            handle.gesture_pinch_update(data, event);
        }
    }
    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        if self.active(data) {
            handle.gesture_pinch_end(data, event);
        }
    }
    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        if self.active(data) {
            handle.gesture_hold_begin(data, event);
        }
    }
    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        if self.active(data) {
            handle.gesture_hold_end(data, event);
        }
    }
    fn start_data(&self) -> &GrabStartData<State> {
        &self.start
    }
    fn unset(&mut self, data: &mut State) {
        data.niri.finish_layer_move(&self.surface);
    }
}
