//! Pure edge geometry and gating used by native pointer edge triggers.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use niri_config::OutputName;
use niri_ipc::Edge;
use smithay::output::Output;
use smithay::utils::{Logical, Point};

use crate::niri::Niri;

#[derive(Debug, Clone)]
enum EdgeAction {
    Event,
    Spawn(Vec<String>),
    SpawnSh(String),
}

/// Returns the nearest configured edge when the logical pointer is in its one-pixel edge band.
pub fn select_edge(size: Option<[f64; 2]>, position: [f64; 2], enabled: [bool; 4]) -> Option<Edge> {
    let [width, height] = size?;
    if !width.is_finite()
        || !height.is_finite()
        || width < 1.
        || height < 1.
        || !position[0].is_finite()
        || !position[1].is_finite()
        || position[0] < 0.
        || position[1] < 0.
        || position[0] > width - 1.
        || position[1] > height - 1.
    {
        return None;
    }

    let distances = [
        (Edge::Left, position[0], enabled[0]),
        (Edge::Right, width - 1. - position[0], enabled[1]),
        (Edge::Top, position[1], enabled[2]),
        (Edge::Bottom, height - 1. - position[1], enabled[3]),
    ];
    distances
        .into_iter()
        .filter(|(_, distance, active)| *active && *distance >= 0. && *distance <= 1.)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(edge, _, _)| edge)
}

/// Checks whether the pointer remains in a given output-local logical edge band.
pub fn is_inside_edge(size: [f64; 2], position: [f64; 2], edge: Edge) -> bool {
    if !size[0].is_finite()
        || !size[1].is_finite()
        || size[0] < 1.
        || size[1] < 1.
        || !position[0].is_finite()
        || !position[1].is_finite()
        || position[0] < 0.
        || position[1] < 0.
        || position[0] > size[0] - 1.
        || position[1] > size[1] - 1.
    {
        return false;
    }
    let distance = match edge {
        Edge::Left => position[0],
        Edge::Right => size[0] - 1. - position[0],
        Edge::Top => position[1],
        Edge::Bottom => size[1] - 1. - position[1],
    };
    (0. ..=1.).contains(&distance)
}

/// Positive movement toward the selected edge, in logical pixels.
pub fn inward_delta(edge: Edge, delta: [f64; 2]) -> f64 {
    match edge {
        Edge::Left => -delta[0],
        Edge::Right => delta[0],
        Edge::Top => -delta[1],
        Edge::Bottom => delta[1],
    }
    .max(0.)
}

/// Portion of relative pointer motion clipped by the output boundary.
pub fn clipped_delta(raw_delta: [f64; 2], actual_delta: [f64; 2]) -> [f64; 2] {
    [
        raw_delta[0] - actual_delta[0],
        raw_delta[1] - actual_delta[1],
    ]
}

/// Returns whether both pressure/velocity gates are satisfied.
pub fn gates_satisfied(
    pressure: f64,
    required_pressure: f64,
    inward_velocity: f64,
    required_velocity: f64,
) -> bool {
    pressure >= required_pressure && inward_velocity >= required_velocity
}

/// Returns whether a dwell interval has elapsed.
pub fn dwell_elapsed(elapsed: Duration, dwell: Duration) -> bool {
    elapsed >= dwell
}

/// Conditions that suppress an edge action globally or on an output without an active monitor.
pub fn is_suppressed(
    overview: bool,
    fullscreen: bool,
    locked: bool,
    inactive_output: bool,
) -> bool {
    overview || fullscreen || locked || inactive_output
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn dwell_gate_waits_and_pressure_velocity_gates_are_combined() {
        assert!(!dwell_elapsed(
            Duration::from_millis(349),
            Duration::from_millis(350)
        ));
        assert!(dwell_elapsed(
            Duration::from_millis(350),
            Duration::from_millis(350)
        ));
        assert!(!gates_satisfied(20., 30., 700., 600.));
        assert!(!gates_satisfied(30., 30., 599., 600.));
        assert!(gates_satisfied(30., 30., 600., 600.));
    }

    #[test]
    fn pressure_counts_only_motion_clipped_at_the_edge() {
        let approach = clipped_delta([-500., 0.], [-497., 0.]);
        assert_eq!(inward_delta(Edge::Left, approach), 3.);
        let tangent = clipped_delta([0., 4.], [0., 4.]);
        assert_eq!(inward_delta(Edge::Left, tangent), 0.);
        let clipped_push = clipped_delta([-2., 0.], [0., 0.]);
        assert_eq!(inward_delta(Edge::Left, clipped_push), 2.);
    }

    #[test]
    fn fullscreen_overview_lock_and_detached_outputs_suppress_triggers() {
        assert!(is_suppressed(false, true, false, false));
        assert!(is_suppressed(true, false, false, false));
        assert!(is_suppressed(false, false, true, false));
        assert!(is_suppressed(false, false, false, true));
        assert!(!is_suppressed(false, false, false, false));
        assert_eq!(select_edge(None, [0., 0.], [true; 4]), None);
    }

    #[test]
    fn scaled_output_edges_use_logical_coordinates() {
        // 2880x1800 physical at 1.5 scale is 1920x1200 logical.
        assert_eq!(
            select_edge(
                Some([1920., 1200.]),
                [1919., 600.],
                [false, true, false, false]
            ),
            Some(Edge::Right)
        );
        assert!(is_inside_edge([1920., 1200.], [1919., 600.], Edge::Right));
    }

    #[test]
    fn configured_nearest_edge_wins_at_corners() {
        assert_eq!(
            select_edge(Some([100., 100.]), [0., 0.], [false, false, true, false]),
            Some(Edge::Top)
        );
        assert_eq!(select_edge(Some([100., 100.]), [0., 0.], [false; 4]), None);
        assert_eq!(
            select_edge(Some([100., 100.]), [0., -20.], [true, false, false, false]),
            None
        );
        assert!(!is_inside_edge([100., 100.], [0., 120.], Edge::Left));
    }

    proptest! {
        #[test]
        fn edge_geometry_and_output_transitions_never_panic(
            width in 500.0f64..100_000.0,
            height in 500.0f64..100_000.0,
            x in -100_000.0f64..100_000.0,
            y in -100_000.0f64..100_000.0,
            scale in prop_oneof![Just(1.0), Just(1.5), Just(2.0), 0.5f64..4.0],
            enabled in any::<[bool; 4]>(),
            output_attached in any::<bool>(),
            native_enabled in any::<bool>(),
        ) {
            let logical_size = [width / scale, height / scale];
            let size = output_attached.then_some(logical_size);
            let edge = native_enabled
                .then(|| select_edge(size, [x, y], enabled))
                .flatten();
            if let Some(edge) = edge {
                let index = match edge {
                    Edge::Left => 0,
                    Edge::Right => 1,
                    Edge::Top => 2,
                    Edge::Bottom => 3,
                };
                prop_assert!(enabled[index]);
                prop_assert!(is_inside_edge(logical_size, [x, y], edge));
            }
            let _ = inward_delta(Edge::Left, [x, y]);
            let _ = is_suppressed(false, false, false, !output_attached);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EdgeTriggerCandidate {
    pub(crate) generation: u64,
    pub(crate) output: String,
    pub(crate) edge: niri_ipc::Edge,
    pub(crate) output_size: [f64; 2],
    action: EdgeAction,
    pub(crate) pressure: f64,
    pub(crate) pressure_required: f64,
    pub(crate) velocity_required: f64,
    pub(crate) velocity_passed: bool,
    pub(crate) armed: bool,
    pub(crate) fired: bool,
    pub(crate) armed_at: Option<Instant>,
    pub(crate) dwell_time: Duration,
}

impl Niri {
    pub(crate) fn clear_edge_trigger(&mut self) {
        if let Some(token) = self.edge_trigger_timer.take() {
            self.event_loop.remove(token);
        }
        self.edge_trigger_candidate = None;
    }

    fn edge_triggers_for_output(&self, output: &Output) -> niri_config::EdgeTriggers {
        let config = self.config.borrow();
        let mut edges = config.edge_triggers.clone();
        if let Some(output_name) = output.user_data().get::<OutputName>() {
            if let Some(output_config) = config.outputs.find(output_name) {
                if let Some(overrides) = &output_config.edge_triggers {
                    niri_config::utils::MergeWith::merge_with(&mut edges, overrides);
                }
            }
        }
        edges
    }

    fn edge_trigger_is_suppressed(&self, output: &Output) -> bool {
        let monitor = self.layout.monitor_for_output(output);
        let inactive_output = monitor.is_none();
        let fullscreen = monitor.is_some_and(|monitor| {
            monitor
                .active_workspace_ref()
                .is_active_pending_fullscreen()
        });
        crate::edge_triggers::is_suppressed(
            self.layout.is_overview_open(),
            fullscreen,
            self.is_locked(),
            inactive_output,
        )
    }

    pub(crate) fn update_edge_trigger(
        &mut self,
        position: Point<f64, Logical>,
        raw_delta: Point<f64, Logical>,
    ) {
        let now = Instant::now();
        let previous = self.edge_trigger_last_motion.replace((position, now));
        let elapsed = previous
            .map(|(_, before)| now.duration_since(before).as_secs_f64())
            .filter(|elapsed| *elapsed > 0.);

        if !self.config.borrow().nacre.edge_triggers || self.is_locked() {
            self.clear_edge_trigger();
            return;
        }

        let Some((output, local)) = self.output_under(position) else {
            self.clear_edge_trigger();
            return;
        };
        if self.edge_trigger_is_suppressed(output) {
            self.clear_edge_trigger();
            return;
        }

        let output_name = output.name();
        let Some(geometry) = self.global_space.output_geometry(output) else {
            self.clear_edge_trigger();
            return;
        };
        let size = geometry.size.to_f64();
        let pointer_position = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or(position);
        let clipped_delta = crate::edge_triggers::clipped_delta(
            [raw_delta.x, raw_delta.y],
            [
                position.x - pointer_position.x,
                position.y - pointer_position.y,
            ],
        );
        let edges = self.edge_triggers_for_output(output);
        let enabled = [
            edges.left.as_ref().is_some_and(|trigger| !trigger.off),
            edges.right.as_ref().is_some_and(|trigger| !trigger.off),
            edges.top.as_ref().is_some_and(|trigger| !trigger.off),
            edges.bottom.as_ref().is_some_and(|trigger| !trigger.off),
        ];
        let Some(edge) =
            crate::edge_triggers::select_edge(Some([size.w, size.h]), [local.x, local.y], enabled)
        else {
            self.clear_edge_trigger();
            return;
        };
        let trigger = match edge {
            niri_ipc::Edge::Left => edges.left.as_ref(),
            niri_ipc::Edge::Right => edges.right.as_ref(),
            niri_ipc::Edge::Top => edges.top.as_ref(),
            niri_ipc::Edge::Bottom => edges.bottom.as_ref(),
        };
        let Some(trigger) = trigger else {
            self.clear_edge_trigger();
            return;
        };

        if self
            .edge_trigger_candidate
            .as_ref()
            .is_some_and(|candidate| {
                candidate.output == output_name && candidate.output_size != [size.w, size.h]
            })
        {
            self.clear_edge_trigger();
        }

        let same_candidate = self
            .edge_trigger_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.output == output_name && candidate.edge == edge);
        if !same_candidate {
            self.clear_edge_trigger();
            self.edge_trigger_generation = self.edge_trigger_generation.wrapping_add(1);
            let generation = self.edge_trigger_generation;
            let pressure_required = trigger.pressure.map_or(0., |pressure| pressure.0);
            let velocity_required = trigger.velocity.map_or(0., |velocity| velocity.0);
            let action = trigger.action.as_ref().map_or(EdgeAction::Event, |action| {
                if let Some(command) = &action.spawn {
                    EdgeAction::Spawn(command.clone())
                } else if let Some(command) = &action.spawn_sh {
                    EdgeAction::SpawnSh(command.clone())
                } else {
                    EdgeAction::Event
                }
            });
            self.edge_trigger_candidate = Some(EdgeTriggerCandidate {
                generation,
                output: output_name,
                edge,
                output_size: [size.w, size.h],
                action,
                pressure: 0.,
                pressure_required,
                velocity_required,
                velocity_passed: velocity_required == 0.,
                armed: false,
                fired: false,
                armed_at: None,
                dwell_time: Duration::from_millis(u64::from(trigger.dwell_time_ms.unwrap_or(250))),
            });
        }

        let Some(candidate) = self.edge_trigger_candidate.as_mut() else {
            return;
        };
        let inward_pressure = crate::edge_triggers::inward_delta(edge, clipped_delta);
        let inward_velocity = crate::edge_triggers::inward_delta(edge, [raw_delta.x, raw_delta.y]);
        candidate.pressure += inward_pressure;
        if candidate.velocity_required > 0. {
            if let Some(elapsed) = elapsed {
                candidate.velocity_passed |=
                    inward_velocity / elapsed >= candidate.velocity_required;
            }
        }
        if !candidate.armed
            && !candidate.fired
            && crate::edge_triggers::gates_satisfied(
                candidate.pressure,
                candidate.pressure_required,
                if candidate.velocity_passed {
                    candidate.velocity_required
                } else {
                    0.
                },
                candidate.velocity_required,
            )
        {
            candidate.armed = true;
            candidate.armed_at = Some(now);
            let generation = candidate.generation;
            let dwell_time = candidate.dwell_time;
            let timer = Timer::from_duration(dwell_time);
            match self.event_loop.insert_source(timer, move |_, _, state| {
                if state
                    .niri
                    .edge_trigger_candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.generation == generation)
                {
                    state.niri.edge_trigger_timer = None;
                    let position = state
                        .niri
                        .seat
                        .get_pointer()
                        .map(|pointer| pointer.current_location());
                    if let Some(position) = position {
                        state.niri.fire_edge_trigger(generation, position);
                    } else {
                        state.niri.clear_edge_trigger();
                    }
                }
                TimeoutAction::Drop
            }) {
                Ok(token) => self.edge_trigger_timer = Some(token),
                Err(err) => {
                    warn!("failed to register edge trigger dwell timer: {err:?}");
                    if let Some(candidate) = self.edge_trigger_candidate.as_mut() {
                        candidate.armed = false;
                    }
                }
            }
        }
    }

    pub(crate) fn fire_edge_trigger(&mut self, generation: u64, position: Point<f64, Logical>) {
        let Some(candidate) = self.edge_trigger_candidate.clone() else {
            return;
        };
        if candidate.generation != generation || !candidate.armed || candidate.fired {
            return;
        }
        if !candidate.armed_at.is_some_and(|armed_at| {
            crate::edge_triggers::dwell_elapsed(armed_at.elapsed(), candidate.dwell_time)
        }) {
            return;
        }

        let Some((output, local)) = self.output_under(position) else {
            self.clear_edge_trigger();
            return;
        };
        let Some(geometry) = self.global_space.output_geometry(output) else {
            self.clear_edge_trigger();
            return;
        };
        let size = geometry.size.to_f64();
        if size.w != candidate.output_size[0] || size.h != candidate.output_size[1] {
            self.clear_edge_trigger();
            return;
        }
        let inside = crate::edge_triggers::is_inside_edge(
            [size.w, size.h],
            [local.x, local.y],
            candidate.edge,
        );
        let edges = self.edge_triggers_for_output(output);
        let enabled = match candidate.edge {
            niri_ipc::Edge::Left => edges.left,
            niri_ipc::Edge::Right => edges.right,
            niri_ipc::Edge::Top => edges.top,
            niri_ipc::Edge::Bottom => edges.bottom,
        }
        .is_some_and(|trigger| !trigger.off);
        if !self.config.borrow().nacre.edge_triggers
            || output.name() != candidate.output
            || !inside
            || !enabled
            || self.edge_trigger_is_suppressed(output)
        {
            self.clear_edge_trigger();
            return;
        }

        match candidate.action {
            EdgeAction::Event => {
                if let Some(server) = self.ipc_server.as_ref() {
                    server.edge_triggered(candidate.output, candidate.edge, [local.x, local.y]);
                }
            }
            EdgeAction::Spawn(command) => {
                crate::utils::spawning::spawn(command, None);
            }
            EdgeAction::SpawnSh(command) => {
                crate::utils::spawning::spawn_sh(command, None);
            }
        }
        if let Some(candidate) = self.edge_trigger_candidate.as_mut() {
            candidate.fired = true;
        }
    }
}

#[cfg(test)]
mod edge_trigger_runtime_tests {
    use std::time::{Duration, Instant};

    use niri_config::{Config, EdgeTrigger, EdgeTriggerAction};
    use smithay::utils::{Logical, Point};

    use super::Niri;
    use crate::tests::Fixture;

    fn configured_fixture() -> Fixture {
        let mut config = Config::default();
        config.edge_triggers.left = Some(EdgeTrigger {
            off: false,
            action: Some(EdgeTriggerAction {
                event: true,
                spawn: None,
                spawn_sh: None,
            }),
            dwell_time_ms: Some(350),
            pressure: None,
            velocity: None,
        });
        let mut fixture = Fixture::with_config(config);
        fixture.add_output(1, (1920, 1080));
        fixture
    }

    fn left_edge_position(fixture: &Fixture, output_idx: u8) -> Point<f64, Logical> {
        let output = fixture.niri_output(output_idx);
        let geometry = fixture
            .state
            .server
            .state
            .niri
            .global_space
            .output_geometry(&output)
            .unwrap();
        geometry.loc.to_f64() + Point::new(0., 50.)
    }

    fn enter_left_edge(niri: &mut Niri, position: Point<f64, Logical>) {
        niri.update_edge_trigger(position, Point::new(-2., 0.));
    }

    #[test]
    fn dwell_fires_once_and_latches_until_pointer_leaves() {
        let mut fixture = configured_fixture();
        let position = left_edge_position(&fixture, 1);
        enter_left_edge(fixture.niri(), position);

        let niri = fixture.niri();
        assert!(niri.edge_trigger_timer.is_some());
        let candidate = niri.edge_trigger_candidate.as_mut().unwrap();
        let generation = candidate.generation;
        candidate.armed_at = Some(Instant::now() - Duration::from_millis(351));
        if let Some(token) = niri.edge_trigger_timer.take() {
            niri.event_loop.remove(token);
        }

        niri.fire_edge_trigger(generation, position);
        assert!(niri
            .edge_trigger_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.fired));
        niri.update_edge_trigger(position, Point::new(-2., 0.));
        assert!(niri.edge_trigger_timer.is_none());

        niri.update_edge_trigger(position + Point::new(20., 0.), Point::new(20., 0.));
        assert!(niri.edge_trigger_candidate.is_none());
    }

    #[test]
    fn overview_and_capability_flag_suppress_runtime_candidates() {
        let mut fixture = configured_fixture();
        let position = left_edge_position(&fixture, 1);
        fixture.niri().layout.toggle_overview();
        enter_left_edge(fixture.niri(), position);
        assert!(fixture.niri().edge_trigger_candidate.is_none());

        fixture.niri().layout.toggle_overview();
        fixture.niri().config.borrow_mut().nacre.edge_triggers = false;
        enter_left_edge(fixture.niri(), position);
        assert!(fixture.niri().edge_trigger_candidate.is_none());
    }

    #[test]
    fn pending_fullscreen_suppresses_edge_trigger_on_its_monitor() {
        let mut fixture = configured_fixture();
        let client = fixture.add_client();
        let surface = {
            let window = fixture.client(client).create_window();
            let surface = window.surface.clone();
            window.commit();
            surface
        };
        fixture.roundtrip(client);
        let window = fixture.client(client).window(&surface);
        window.attach_new_buffer();
        window.set_size(100, 100);
        window.ack_last_and_commit();
        fixture.double_roundtrip(client);

        let window_id = fixture
            .niri()
            .layout
            .windows()
            .next()
            .unwrap()
            .1
            .window
            .clone();
        fixture.niri().layout.set_fullscreen(&window_id, true);
        let position = left_edge_position(&fixture, 1);
        enter_left_edge(fixture.niri(), position);
        assert!(fixture.niri().edge_trigger_candidate.is_none());

        fixture.niri().layout.set_fullscreen(&window_id, false);
        enter_left_edge(fixture.niri(), position);
        assert!(fixture.niri().edge_trigger_candidate.is_some());
    }

    #[test]
    fn output_removal_and_config_reload_clear_pending_dwell() {
        let mut fixture = configured_fixture();
        fixture.add_output(2, (1280, 720));
        let first_position = left_edge_position(&fixture, 1);
        enter_left_edge(fixture.niri(), first_position);
        let position = left_edge_position(&fixture, 2);
        enter_left_edge(fixture.niri(), position);
        assert!(fixture.niri().edge_trigger_timer.is_some());
        let second_output_name = fixture.niri_output(2).name();
        assert_eq!(
            fixture
                .niri()
                .edge_trigger_candidate
                .as_ref()
                .map(|candidate| candidate.output.as_str()),
            Some(second_output_name.as_str())
        );

        let output = fixture.niri_output(2);
        fixture.niri().remove_output(&output);
        assert!(fixture.niri().edge_trigger_candidate.is_none());
        assert!(fixture.niri().edge_trigger_timer.is_none());

        let position = left_edge_position(&fixture, 1);
        enter_left_edge(fixture.niri(), position);
        assert!(fixture.niri().edge_trigger_candidate.is_some());
        fixture.niri_state().reload_config(Ok(Config::default()));
        assert!(fixture.niri().edge_trigger_candidate.is_none());
        assert!(fixture.niri().edge_trigger_timer.is_none());
    }
}
