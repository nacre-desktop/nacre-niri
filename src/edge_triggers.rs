//! Pure edge geometry and gating used by native pointer edge triggers.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use niri_config::OutputName;
use niri_ipc::Edge;
use smithay::output::Output;
use smithay::utils::{Logical, Point};

use crate::niri::Niri;

/// Whether the native edge event source can emit any configured IPC events.
///
/// Edges without an explicit spawn action default to IPC events, matching the
/// runtime action selection below. Output overrides are merged with global definitions using the
/// same path as the runtime edge selection.
pub(crate) fn event_source_available(config: &niri_config::Config) -> bool {
    config.nacre.edge_triggers
        && (has_event_action(&config.edge_triggers)
            || config.outputs.0.iter().any(|output| {
                let mut edges = config.edge_triggers.clone();
                if let Some(overrides) = &output.edge_triggers {
                    niri_config::utils::MergeWith::merge_with(&mut edges, overrides);
                }
                has_event_action(&edges)
            }))
}

fn has_event_action(edges: &niri_config::EdgeTriggers) -> bool {
    [&edges.left, &edges.right, &edges.top, &edges.bottom]
        .into_iter()
        .any(|trigger| {
            trigger.as_ref().is_some_and(|trigger| {
                !trigger.off
                    && trigger
                        .action
                        .as_ref()
                        .is_none_or(|action| action.spawn.is_none() && action.spawn_sh.is_none())
            })
        })
}

fn edge_band_width(trigger: Option<&niri_config::EdgeTrigger>) -> Option<f64> {
    trigger
        .and_then(|trigger| trigger.trigger_distance)
        .map(|distance| distance.0)
}

fn edge_span_width(trigger: Option<&niri_config::EdgeTrigger>) -> Option<f64> {
    trigger
        .and_then(|trigger| trigger.trigger_length)
        .map(|length| length.0)
}

fn cue_frame_interval(refresh_millihz: u32) -> Duration {
    if refresh_millihz == 0 {
        Duration::from_nanos(16_666_667)
    } else {
        Duration::from_secs_f64(1_000. / f64::from(refresh_millihz))
    }
}

/// Whether any effective event edge opts in to pre-dwell hover cues.
pub(crate) fn cue_source_available(config: &niri_config::Config) -> bool {
    config.nacre.edge_triggers
        && ((config.edge_triggers.cue && has_event_action(&config.edge_triggers))
            || config.outputs.0.iter().any(|output| {
                let mut edges = config.edge_triggers.clone();
                if let Some(overrides) = &output.edge_triggers {
                    niri_config::utils::MergeWith::merge_with(&mut edges, overrides);
                }
                edges.cue && has_event_action(&edges)
            }))
}

#[derive(Debug, Clone)]
enum EdgeAction {
    Event,
    Spawn(Vec<String>),
    SpawnSh(String),
}

/// Returns the nearest configured edge when the logical pointer is in its one-pixel edge band.
#[cfg(test)]
pub fn select_edge(size: Option<[f64; 2]>, position: [f64; 2], enabled: [bool; 4]) -> Option<Edge> {
    select_edge_with_distances(size, position, enabled, [None; 4])
}

/// Returns the nearest configured edge using each edge's own trigger-band width.
pub fn select_edge_with_distances(
    size: Option<[f64; 2]>,
    position: [f64; 2],
    enabled: [bool; 4],
    distances: [Option<f64>; 4],
) -> Option<Edge> {
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

    let edge_distances = [
        (Edge::Left, position[0], enabled[0], distances[0], width),
        (Edge::Right, position[0], enabled[1], distances[1], width),
        (Edge::Top, position[1], enabled[2], distances[2], height),
        (Edge::Bottom, position[1], enabled[3], distances[3], height),
    ];
    edge_distances
        .into_iter()
        .filter(|(edge, coordinate, active, trigger_distance, extent)| {
            let inside = match trigger_distance {
                Some(distance) => {
                    distance.is_finite()
                        && *distance > 0.
                        && match edge {
                            Edge::Left | Edge::Top => *coordinate < *distance,
                            Edge::Right | Edge::Bottom => *coordinate >= *extent - *distance,
                        }
                }
                None => {
                    let edge_distance = match edge {
                        Edge::Left | Edge::Top => *coordinate,
                        Edge::Right | Edge::Bottom => *extent - 1. - *coordinate,
                    };
                    (0. ..=1.).contains(&edge_distance)
                }
            };
            *active && inside
        })
        .min_by(|a, b| {
            let edge_distance = |(edge, coordinate, _, trigger_distance, extent): &(
                Edge,
                f64,
                bool,
                Option<f64>,
                f64,
            )| {
                match (edge, trigger_distance) {
                    (Edge::Left | Edge::Top, _) => *coordinate,
                    (Edge::Right | Edge::Bottom, Some(_)) => *extent - *coordinate,
                    (Edge::Right | Edge::Bottom, None) => *extent - 1. - *coordinate,
                }
            };
            edge_distance(a).total_cmp(&edge_distance(b))
        })
        .map(|(edge, _, _, _, _)| edge)
}

/// Checks whether the pointer remains in a given output-local logical edge band.
#[cfg(test)]
pub fn is_inside_edge(size: [f64; 2], position: [f64; 2], edge: Edge) -> bool {
    is_inside_edge_with_distance(size, position, edge, None)
}

/// Checks whether the pointer remains in a given output-local trigger band.
pub fn is_inside_edge_with_distance(
    size: [f64; 2],
    position: [f64; 2],
    edge: Edge,
    distance: Option<f64>,
) -> bool {
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
    match distance {
        Some(distance) if distance.is_finite() && distance > 0. => match edge {
            Edge::Left => position[0] < distance,
            Edge::Right => position[0] >= size[0] - distance,
            Edge::Top => position[1] < distance,
            Edge::Bottom => position[1] >= size[1] - distance,
        },
        Some(_) => false,
        None => {
            let edge_distance = match edge {
                Edge::Left => position[0],
                Edge::Right => size[0] - 1. - position[0],
                Edge::Top => position[1],
                Edge::Bottom => size[1] - 1. - position[1],
            };
            (0. ..=1.).contains(&edge_distance)
        }
    }
}

/// Checks whether the pointer is within a centered along-edge span, when one is configured.
pub fn is_inside_edge_span(
    size: [f64; 2],
    position: [f64; 2],
    edge: Edge,
    length: Option<f64>,
) -> bool {
    let Some(length) = length else {
        return true;
    };
    if !length.is_finite() || length <= 0. {
        return false;
    }
    let (extent, coordinate) = match edge {
        Edge::Left | Edge::Right => (size[1], position[1]),
        Edge::Top | Edge::Bottom => (size[0], position[0]),
    };
    if !extent.is_finite() || extent <= 0. || !coordinate.is_finite() {
        return false;
    }
    let length = length.min(extent);
    let start = (extent - length) / 2.;
    let end = (extent + length) / 2.;
    coordinate >= start && coordinate < end
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

/// Converts a global logical position to coordinates local to a previously attached output.
pub fn output_local_position(global: [f64; 2], origin: [f64; 2]) -> [f64; 2] {
    [global[0] - origin[0], global[1] - origin[1]]
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
    pointer_grabbed: bool,
) -> bool {
    overview || fullscreen || locked || inactive_output || pointer_grabbed
}

/// Whether pointer motion has an unambiguous accepted displacement for edge tracking.
pub fn pointer_motion_is_eligible(pointer_confined: bool, spatial_wrapping: bool) -> bool {
    !pointer_confined && !spatial_wrapping
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use niri_config::{Config, EdgeTrigger, EdgeTriggerAction, EdgeTriggersPart, Output};
    use proptest::prelude::*;

    use super::*;

    fn event_trigger() -> EdgeTrigger {
        EdgeTrigger {
            off: false,
            action: Some(EdgeTriggerAction {
                event: true,
                spawn: None,
                spawn_sh: None,
            }),
            dwell_time_ms: None,
            pressure: None,
            velocity: None,
            trigger_distance: None,
            trigger_length: None,
        }
    }

    #[test]
    fn event_source_requires_an_enabled_event_action() {
        let mut config = Config::default();
        assert!(!event_source_available(&config));

        config.edge_triggers.left = Some(event_trigger());
        config.nacre.edge_triggers = false;
        assert!(!event_source_available(&config));
        config.nacre.edge_triggers = true;
        config.edge_triggers.left.as_mut().unwrap().off = true;
        assert!(!event_source_available(&config));

        config.edge_triggers.left = Some(EdgeTrigger {
            action: Some(EdgeTriggerAction {
                event: false,
                spawn: Some(vec![String::from("panel-command")]),
                spawn_sh: None,
            }),
            ..event_trigger()
        });
        assert!(!event_source_available(&config));

        config.edge_triggers.left = Some(event_trigger());
        assert!(event_source_available(&config));

        config.edge_triggers.left.as_mut().unwrap().action = None;
        assert!(event_source_available(&config));

        config.edge_triggers.left = Some(EdgeTrigger {
            action: Some(EdgeTriggerAction {
                event: false,
                spawn: Some(vec![String::from("panel-command")]),
                spawn_sh: None,
            }),
            ..event_trigger()
        });
        let mut output = Output {
            name: String::from("DP-1"),
            edge_triggers: Some(EdgeTriggersPart::default()),
            ..Output::default()
        };
        output.edge_triggers.as_mut().unwrap().left = Some(EdgeTrigger {
            action: None,
            ..event_trigger()
        });
        config.outputs.0.push(output);
        assert!(event_source_available(&config));

        config.outputs.0[0].edge_triggers.as_mut().unwrap().left = Some(event_trigger());
        assert!(event_source_available(&config));
    }

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
        assert!(is_suppressed(false, true, false, false, false));
        assert!(is_suppressed(true, false, false, false, false));
        assert!(is_suppressed(false, false, true, false, false));
        assert!(is_suppressed(false, false, false, true, false));
        assert!(is_suppressed(false, false, false, false, true));
        assert!(!is_suppressed(false, false, false, false, false));
        assert_eq!(select_edge(None, [0., 0.], [true; 4]), None);
        assert!(!pointer_motion_is_eligible(true, false));
        assert!(!pointer_motion_is_eligible(false, true));
        assert!(pointer_motion_is_eligible(false, false));
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
        assert_eq!(
            output_local_position([1932., 40.], [1920., 0.]),
            [12.0, 40.0]
        );
    }

    #[test]
    fn configured_trigger_distances_use_half_open_lip_boundaries() {
        let size = [100., 80.];
        let enabled = [true; 4];
        let distances = [Some(10.); 4];
        assert_eq!(
            select_edge_with_distances(Some(size), [9.9, 40.], enabled, distances),
            Some(Edge::Left)
        );
        assert_eq!(
            select_edge_with_distances(Some(size), [10., 40.], enabled, distances),
            None
        );
        assert_eq!(
            select_edge_with_distances(Some(size), [89.9, 40.], enabled, distances),
            None
        );
        assert_eq!(
            select_edge_with_distances(Some(size), [90., 40.], enabled, distances),
            Some(Edge::Right)
        );
        assert_eq!(
            select_edge_with_distances(Some(size), [50., 9.9], enabled, distances),
            Some(Edge::Top)
        );
        assert_eq!(
            select_edge_with_distances(Some(size), [50., 10.], enabled, distances),
            None
        );
        assert_eq!(
            select_edge_with_distances(Some(size), [50., 69.9], enabled, distances),
            None
        );
        assert_eq!(
            select_edge_with_distances(Some(size), [50., 70.], enabled, distances),
            Some(Edge::Bottom)
        );
        assert_eq!(
            select_edge_with_distances(Some(size), [0., 40.], enabled, [Some(1.); 4]),
            Some(Edge::Left)
        );
        assert!(!is_inside_edge_with_distance(
            size,
            [10., 40.],
            Edge::Left,
            Some(10.)
        ));
        assert!(is_inside_edge_with_distance(
            size,
            [9.9, 40.],
            Edge::Left,
            Some(10.)
        ));
    }

    #[test]
    fn configured_edge_spans_are_centered_half_open_and_clipped_to_small_outputs() {
        let size = [100., 80.];
        assert!(!is_inside_edge_span(
            size,
            [0., 29.9],
            Edge::Left,
            Some(20.)
        ));
        assert!(is_inside_edge_span(size, [0., 30.], Edge::Left, Some(20.)));
        assert!(is_inside_edge_span(
            size,
            [99., 49.9],
            Edge::Right,
            Some(20.)
        ));
        assert!(!is_inside_edge_span(
            size,
            [99., 50.],
            Edge::Right,
            Some(20.)
        ));
        assert!(!is_inside_edge_span(size, [29.9, 0.], Edge::Top, Some(40.)));
        assert!(is_inside_edge_span(size, [30., 0.], Edge::Top, Some(40.)));
        assert!(is_inside_edge_span(
            size,
            [69.9, 79.],
            Edge::Bottom,
            Some(40.)
        ));
        assert!(!is_inside_edge_span(
            size,
            [70., 79.],
            Edge::Bottom,
            Some(40.)
        ));
        assert!(is_inside_edge_span(
            [100., 12.],
            [0., 11.9],
            Edge::Left,
            Some(144.)
        ));
        assert!(!is_inside_edge_span(
            [100., 12.],
            [0., 12.],
            Edge::Left,
            Some(144.)
        ));
        assert!(is_inside_edge_span(size, [0., 79.], Edge::Left, None));
    }

    #[test]
    fn cue_interval_uses_refresh_and_falls_back_to_60hz_without_a_mode_rate() {
        assert!((16_666_666..=16_666_667).contains(&cue_frame_interval(60_000).as_nanos()));
        assert!((6_944_444..=6_944_445).contains(&cue_frame_interval(144_000).as_nanos()));
        assert_eq!(cue_frame_interval(0), Duration::from_nanos(16_666_667));
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
            let _ = is_suppressed(false, false, false, !output_attached, false);
            let local = [width / scale, height / scale];
            let origin = [x, y];
            let global = [origin[0] + local[0], origin[1] + local[1]];
            let reported = output_local_position(global, origin);
            prop_assert!((reported[0] - local[0]).abs() < 1e-8);
            prop_assert!((reported[1] - local[1]).abs() < 1e-8);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EdgeTriggerCandidate {
    pub(crate) generation: u64,
    pub(crate) output: String,
    pub(crate) edge: niri_ipc::Edge,
    pub(crate) output_size: [f64; 2],
    output_origin: [f64; 2],
    last_position: [f64; 2],
    action: EdgeAction,
    pub(crate) pressure: f64,
    pub(crate) pressure_required: f64,
    pub(crate) velocity_required: f64,
    pub(crate) velocity_passed: bool,
    pub(crate) armed: bool,
    pub(crate) fired: bool,
    pub(crate) armed_at: Option<Instant>,
    pub(crate) dwell_time: Duration,
    trigger_distance: Option<f64>,
    trigger_length: Option<f64>,
    cue: bool,
    cue_interval: Duration,
    last_cue_position: Option<[f64; 2]>,
    last_cue_progress: Option<f64>,
}

/// Runs on every pointer motion: borrow the global config and clone only for an output override.
fn edge_triggers_for_output<'a>(
    config: &'a niri_config::Config,
    output: &Output,
) -> Cow<'a, niri_config::EdgeTriggers> {
    let overrides = output
        .user_data()
        .get::<OutputName>()
        .and_then(|name| config.outputs.find(name))
        .and_then(|output_config| output_config.edge_triggers.as_ref());
    match overrides {
        Some(overrides) => {
            let mut edges = config.edge_triggers.clone();
            niri_config::utils::MergeWith::merge_with(&mut edges, overrides);
            Cow::Owned(edges)
        }
        None => Cow::Borrowed(&config.edge_triggers),
    }
}

impl Niri {
    pub(crate) fn clear_edge_trigger(&mut self) {
        let current_position = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location());
        self.clear_edge_trigger_at(current_position);
    }

    fn clear_edge_trigger_at(&mut self, position: Option<Point<f64, Logical>>) {
        self.clear_edge_trigger_at_inner(position, true);
    }

    fn clear_edge_trigger_at_inner(
        &mut self,
        position: Option<Point<f64, Logical>>,
        remove_timer: bool,
    ) {
        if remove_timer {
            if let Some(token) = self.edge_trigger_timer.take() {
                self.event_loop.remove(token);
            }
        } else {
            self.edge_trigger_timer = None;
        }
        let Some(candidate) = self.edge_trigger_candidate.take() else {
            return;
        };
        if candidate.cue || (candidate.fired && matches!(&candidate.action, &EdgeAction::Event)) {
            let position = position.unwrap_or_else(|| {
                Point::new(candidate.last_position[0], candidate.last_position[1])
            });
            if let Some(server) = self.ipc_server.as_ref() {
                server.edge_left(
                    candidate.output,
                    candidate.edge,
                    crate::edge_triggers::output_local_position(
                        [position.x, position.y],
                        candidate.output_origin,
                    ),
                );
            }
        }
    }

    fn edge_trigger_cue_tick(
        &mut self,
        generation: u64,
        position: Point<f64, Logical>,
        from_timer: bool,
    ) -> bool {
        let Some(candidate) = self
            .edge_trigger_candidate
            .as_ref()
            .filter(|candidate| candidate.generation == generation)
            .cloned()
        else {
            return false;
        };
        let Some((output, local)) = self.output_under(position) else {
            self.clear_edge_trigger_at_inner(Some(position), !from_timer);
            return false;
        };
        let Some(geometry) = self.global_space.output_geometry(output) else {
            self.clear_edge_trigger_at_inner(Some(position), !from_timer);
            return false;
        };
        let size = geometry.size.to_f64();
        if output.name() != candidate.output
            || size.w != candidate.output_size[0]
            || size.h != candidate.output_size[1]
            || !crate::edge_triggers::is_inside_edge_with_distance(
                [size.w, size.h],
                [local.x, local.y],
                candidate.edge,
                candidate.trigger_distance,
            )
            || !crate::edge_triggers::is_inside_edge_span(
                [size.w, size.h],
                [local.x, local.y],
                candidate.edge,
                candidate.trigger_length,
            )
            || self.edge_trigger_is_suppressed(output)
        {
            self.clear_edge_trigger_at_inner(Some(position), !from_timer);
            return false;
        }

        let progress = if candidate.fired {
            1.
        } else if candidate.armed {
            candidate
                .armed_at
                .map(|at| {
                    if candidate.dwell_time.is_zero() {
                        1.
                    } else {
                        (at.elapsed().as_secs_f64() / candidate.dwell_time.as_secs_f64())
                            .clamp(0., 1.)
                    }
                })
                .unwrap_or(0.)
        } else {
            0.
        };
        let local_position = [local.x, local.y];
        if candidate.last_cue_position != Some(local_position)
            || candidate.last_cue_progress != Some(progress)
        {
            if let Some(server) = self.ipc_server.as_ref() {
                server.edge_hover(
                    candidate.output.clone(),
                    candidate.edge,
                    local_position,
                    progress,
                );
            }
            if let Some(current) = self.edge_trigger_candidate.as_mut() {
                current.last_cue_position = Some(local_position);
                current.last_cue_progress = Some(progress);
            }
        }
        if !candidate.fired && candidate.armed {
            self.fire_edge_trigger(generation, position);
        }
        true
    }

    fn edge_trigger_is_suppressed(&self, output: &Output) -> bool {
        let monitor = self.layout.monitor_for_output(output);
        let inactive_output = monitor.is_none();
        let fullscreen = monitor.is_some_and(|monitor| {
            monitor
                .active_workspace_ref()
                .is_active_pending_fullscreen()
        });
        let pointer_grabbed = self
            .seat
            .get_pointer()
            .is_some_and(|pointer| pointer.is_grabbed());
        crate::edge_triggers::is_suppressed(
            self.layout.is_overview_open(),
            fullscreen,
            self.is_locked(),
            inactive_output,
            pointer_grabbed,
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
            self.clear_edge_trigger_at(Some(position));
            return;
        }

        let Some((output, local)) = self.output_under(position) else {
            self.clear_edge_trigger_at(Some(position));
            return;
        };
        if self.edge_trigger_is_suppressed(output) {
            self.clear_edge_trigger_at(Some(position));
            return;
        }

        let Some(geometry) = self.global_space.output_geometry(output) else {
            self.clear_edge_trigger_at(Some(position));
            return;
        };
        let size = geometry.size.to_f64();
        let output_origin = [geometry.loc.x as f64, geometry.loc.y as f64];
        let cue_interval = cue_frame_interval(
            output
                .current_mode()
                .map(|mode| mode.refresh.try_into().unwrap_or(0))
                .unwrap_or(60_000),
        );
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
        // Keep the config borrow local so the `&mut self` calls below stay valid.
        let config = self.config.clone();
        let config = config.borrow();
        let edges = edge_triggers_for_output(&config, output);
        let edge_lengths = [
            edge_span_width(edges.left.as_ref()),
            edge_span_width(edges.right.as_ref()),
            edge_span_width(edges.top.as_ref()),
            edge_span_width(edges.bottom.as_ref()),
        ];
        let enabled = [
            edges.left.as_ref().is_some_and(|trigger| !trigger.off)
                && crate::edge_triggers::is_inside_edge_span(
                    [size.w, size.h],
                    [local.x, local.y],
                    niri_ipc::Edge::Left,
                    edge_lengths[0],
                ),
            edges.right.as_ref().is_some_and(|trigger| !trigger.off)
                && crate::edge_triggers::is_inside_edge_span(
                    [size.w, size.h],
                    [local.x, local.y],
                    niri_ipc::Edge::Right,
                    edge_lengths[1],
                ),
            edges.top.as_ref().is_some_and(|trigger| !trigger.off)
                && crate::edge_triggers::is_inside_edge_span(
                    [size.w, size.h],
                    [local.x, local.y],
                    niri_ipc::Edge::Top,
                    edge_lengths[2],
                ),
            edges.bottom.as_ref().is_some_and(|trigger| !trigger.off)
                && crate::edge_triggers::is_inside_edge_span(
                    [size.w, size.h],
                    [local.x, local.y],
                    niri_ipc::Edge::Bottom,
                    edge_lengths[3],
                ),
        ];
        let edge_distances = [
            edge_band_width(edges.left.as_ref()),
            edge_band_width(edges.right.as_ref()),
            edge_band_width(edges.top.as_ref()),
            edge_band_width(edges.bottom.as_ref()),
        ];
        let Some(edge) = crate::edge_triggers::select_edge_with_distances(
            Some([size.w, size.h]),
            [local.x, local.y],
            enabled,
            edge_distances,
        ) else {
            self.clear_edge_trigger_at(Some(position));
            return;
        };
        let trigger = match edge {
            niri_ipc::Edge::Left => edges.left.as_ref(),
            niri_ipc::Edge::Right => edges.right.as_ref(),
            niri_ipc::Edge::Top => edges.top.as_ref(),
            niri_ipc::Edge::Bottom => edges.bottom.as_ref(),
        };
        let Some(trigger) = trigger else {
            self.clear_edge_trigger_at(Some(position));
            return;
        };

        let output_name = output.name();
        if self
            .edge_trigger_candidate
            .as_ref()
            .is_some_and(|candidate| {
                candidate.output == output_name && candidate.output_size != [size.w, size.h]
            })
        {
            self.clear_edge_trigger_at(Some(position));
        }

        let same_candidate = self
            .edge_trigger_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.output == output_name && candidate.edge == edge);
        if !same_candidate {
            self.clear_edge_trigger_at(Some(position));
            self.edge_trigger_generation = self.edge_trigger_generation.wrapping_add(1);
            let generation = self.edge_trigger_generation;
            let pressure_required = trigger.pressure.map_or(0., |pressure| pressure.0);
            let velocity_required = trigger.velocity.map_or(0., |velocity| velocity.0);
            let cue = edges.cue
                && trigger
                    .action
                    .as_ref()
                    .is_none_or(|action| action.spawn.is_none() && action.spawn_sh.is_none());
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
                output: output_name.clone(),
                edge,
                output_size: [size.w, size.h],
                output_origin,
                last_position: [position.x, position.y],
                action,
                pressure: 0.,
                pressure_required,
                velocity_required,
                velocity_passed: velocity_required == 0.,
                armed: false,
                fired: false,
                armed_at: None,
                dwell_time: Duration::from_millis(u64::from(trigger.dwell_time_ms.unwrap_or(250))),
                trigger_distance: edge_band_width(Some(trigger)),
                trigger_length: edge_span_width(Some(trigger)),
                cue,
                cue_interval,
                last_cue_position: None,
                last_cue_progress: None,
            });
            if cue {
                if let Some(server) = self.ipc_server.as_ref() {
                    server.edge_approach(output_name.clone(), edge, [local.x, local.y]);
                }
                if let Some(candidate) = self.edge_trigger_candidate.as_mut() {
                    candidate.last_cue_position = Some([local.x, local.y]);
                    candidate.last_cue_progress = Some(0.);
                }
                let cue_interval = self.edge_trigger_candidate.as_ref().unwrap().cue_interval;
                let timer = Timer::from_duration(cue_interval);
                match self.event_loop.insert_source(timer, move |_, _, state| {
                    let Some(candidate) = state
                        .niri
                        .edge_trigger_candidate
                        .as_ref()
                        .filter(|candidate| candidate.generation == generation)
                        .cloned()
                    else {
                        return TimeoutAction::Drop;
                    };
                    let Some(position) = state
                        .niri
                        .seat
                        .get_pointer()
                        .map(|pointer| pointer.current_location())
                    else {
                        state.niri.clear_edge_trigger_at_inner(None, false);
                        return TimeoutAction::Drop;
                    };
                    if state.niri.edge_trigger_cue_tick(generation, position, true) {
                        TimeoutAction::ToDuration(candidate.cue_interval)
                    } else {
                        TimeoutAction::Drop
                    }
                }) {
                    Ok(token) => self.edge_trigger_timer = Some(token),
                    Err(err) => {
                        warn!("failed to register edge cue timer: {err:?}");
                        self.clear_edge_trigger_at(Some(position));
                        return;
                    }
                }
            }
        }

        let Some(candidate) = self.edge_trigger_candidate.as_mut() else {
            return;
        };
        candidate.last_position = [position.x, position.y];
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
            if self
                .edge_trigger_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.cue)
            {
                return;
            }
            let timer = Timer::from_duration(dwell_time);
            match self.event_loop.insert_source(timer, move |_, _, state| {
                let Some(candidate) = state
                    .niri
                    .edge_trigger_candidate
                    .as_ref()
                    .filter(|candidate| candidate.generation == generation)
                    .cloned()
                else {
                    return TimeoutAction::Drop;
                };
                state.niri.edge_trigger_timer = None;
                let Some(position) = state
                    .niri
                    .seat
                    .get_pointer()
                    .map(|pointer| pointer.current_location())
                else {
                    state.niri.clear_edge_trigger_at_inner(None, false);
                    return TimeoutAction::Drop;
                };
                if !candidate.fired && candidate.armed {
                    state.niri.fire_edge_trigger(generation, position);
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
            self.clear_edge_trigger_at(Some(position));
            return;
        };
        let Some(geometry) = self.global_space.output_geometry(output) else {
            self.clear_edge_trigger_at(Some(position));
            return;
        };
        let size = geometry.size.to_f64();
        if size.w != candidate.output_size[0] || size.h != candidate.output_size[1] {
            self.clear_edge_trigger_at(Some(position));
            return;
        }
        let inside = crate::edge_triggers::is_inside_edge_with_distance(
            [size.w, size.h],
            [local.x, local.y],
            candidate.edge,
            candidate.trigger_distance,
        );
        let inside_span = crate::edge_triggers::is_inside_edge_span(
            [size.w, size.h],
            [local.x, local.y],
            candidate.edge,
            candidate.trigger_length,
        );
        let config = self.config.clone();
        let config = config.borrow();
        let edges = edge_triggers_for_output(&config, output);
        let enabled = match candidate.edge {
            niri_ipc::Edge::Left => &edges.left,
            niri_ipc::Edge::Right => &edges.right,
            niri_ipc::Edge::Top => &edges.top,
            niri_ipc::Edge::Bottom => &edges.bottom,
        }
        .as_ref()
        .is_some_and(|trigger| !trigger.off);
        if !config.nacre.edge_triggers
            || output.name() != candidate.output
            || !inside
            || !inside_span
            || !enabled
            || self.edge_trigger_is_suppressed(output)
        {
            self.clear_edge_trigger_at(Some(position));
            return;
        }

        match candidate.action {
            EdgeAction::Event => {
                if let Some(server) = self.ipc_server.as_ref() {
                    server.edge_triggered(candidate.output, candidate.edge, [local.x, local.y]);
                }
            }
            EdgeAction::Spawn(command) => {
                let (token, gate) = self.prepare_spawn_launch();
                crate::utils::spawning::spawn_with_launch(command, token, gate);
            }
            EdgeAction::SpawnSh(command) => {
                let (token, gate) = self.prepare_spawn_launch();
                crate::utils::spawning::spawn_sh_with_launch(command, token, gate);
            }
        }
        if let Some(candidate) = self.edge_trigger_candidate.as_mut() {
            candidate.last_position = [position.x, position.y];
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

    fn configured_fixture_with_action(action: EdgeTriggerAction) -> Fixture {
        let mut config = Config::default();
        config.edge_triggers.left = Some(EdgeTrigger {
            off: false,
            action: Some(action),
            dwell_time_ms: Some(350),
            pressure: None,
            velocity: None,
            trigger_distance: None,
            trigger_length: None,
        });
        let mut fixture = Fixture::with_config(config);
        fixture.add_output(1, (1920, 1080));
        fixture
    }

    fn configured_fixture() -> Fixture {
        configured_fixture_with_action(EdgeTriggerAction {
            event: true,
            spawn: None,
            spawn_sh: None,
        })
    }

    fn cue_configured_fixture() -> Fixture {
        let mut config = Config::default();
        config.edge_triggers.cue = true;
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
            trigger_distance: Some(niri_config::FloatOrInt(10.)),
            trigger_length: None,
        });
        let mut fixture = Fixture::with_config(config);
        fixture.add_output(1, (1920, 1080));
        fixture
    }

    #[test]
    fn centered_lip_span_gates_candidate_and_reentry_restarts_approach() {
        let mut fixture = cue_configured_fixture();
        fixture
            .niri()
            .config
            .borrow_mut()
            .edge_triggers
            .left
            .as_mut()
            .unwrap()
            .trigger_length = Some(niri_config::FloatOrInt(144.));
        let events = attach_edge_events(&mut fixture);
        let outside = left_edge_position(&fixture, 1) + Point::new(5., 0.);
        enter_left_edge(fixture.niri(), outside);
        assert!(fixture.niri().edge_trigger_candidate.is_none());
        assert!(
            events.try_recv().is_err(),
            "outside the centered lip must not approach"
        );

        let center = left_edge_position(&fixture, 1) + Point::new(5., 490.);
        enter_left_edge(fixture.niri(), center);
        let Ok(niri_ipc::Event::EdgeApproach {
            progress, position, ..
        }) = events.try_recv()
        else {
            panic!("entering the centered lip did not emit EdgeApproach");
        };
        assert_eq!(progress, 0.);
        assert_eq!(position, [5., 540.]);
        let first_candidate = fixture.niri().edge_trigger_candidate.as_ref().unwrap();
        let first_generation = first_candidate.generation;
        let first_armed_at = first_candidate.armed_at.unwrap();

        let left_span = center + Point::new(0., -72.1);
        fixture
            .niri()
            .update_edge_trigger(left_span, Point::new(0., -72.1));
        assert!(matches!(
            events.try_recv(),
            Ok(niri_ipc::Event::EdgeLeft { .. })
        ));
        assert!(fixture.niri().edge_trigger_candidate.is_none());

        let reentered = center + Point::new(0., -72.);
        enter_left_edge(fixture.niri(), reentered);
        let Ok(niri_ipc::Event::EdgeApproach {
            progress, position, ..
        }) = events.try_recv()
        else {
            panic!("re-entering the centered lip did not start a fresh approach");
        };
        assert_eq!(progress, 0.);
        assert_eq!(position, [5., 468.]);
        let niri = fixture.niri();
        let candidate = niri.edge_trigger_candidate.as_ref().unwrap();
        let reentry_generation = candidate.generation;
        let reentry_armed_at = candidate.armed_at.unwrap();
        assert_ne!(reentry_generation, first_generation);
        assert!(reentry_armed_at > first_armed_at);
        niri.edge_trigger_candidate.as_mut().unwrap().armed_at =
            Some(Instant::now() - Duration::from_millis(10));
        assert!(niri.edge_trigger_cue_tick(reentry_generation, reentered, false));
        let Ok(niri_ipc::Event::EdgeHover { progress, .. }) = events.try_recv() else {
            panic!("fresh candidate did not publish dwell progress");
        };
        assert!((0.01..0.1).contains(&progress));
        assert!(
            events.try_recv().is_err(),
            "re-entry must not trigger immediately"
        );
    }

    fn attach_edge_events(fixture: &mut Fixture) -> async_channel::Receiver<niri_ipc::Event> {
        let server = crate::ipc::server::IpcServer::start(&fixture.niri().event_loop, None)
            .expect("headless edge IPC server");
        let events = server.subscribe_edge_events();
        fixture.niri().ipc_server = Some(server);
        events
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
    fn cue_tick_reports_stationary_progress_trigger_and_unfired_leave() {
        let mut fixture = cue_configured_fixture();
        let events = attach_edge_events(&mut fixture);
        let position = left_edge_position(&fixture, 1) + Point::new(5., 0.);
        enter_left_edge(fixture.niri(), position);
        let Ok(niri_ipc::Event::EdgeApproach {
            output,
            edge,
            position: approach_position,
            progress: approach_progress,
        }) = events.try_recv()
        else {
            panic!("cue entry did not publish EdgeApproach");
        };
        assert_eq!(output, "headless-1");
        assert_eq!(edge, niri_ipc::Edge::Left);
        assert_eq!(approach_position, [5.0, 50.0]);
        assert_eq!(approach_progress, 0.);

        let niri = fixture.niri();
        let generation = {
            let candidate = niri.edge_trigger_candidate.as_mut().unwrap();
            candidate.armed_at = Some(Instant::now() - Duration::from_millis(175));
            candidate.generation
        };
        if let Some(token) = niri.edge_trigger_timer.take() {
            niri.event_loop.remove(token);
        }
        assert!(niri.edge_trigger_cue_tick(generation, position, false));
        let Ok(niri_ipc::Event::EdgeHover {
            progress,
            position: hover_position,
            ..
        }) = events.try_recv()
        else {
            panic!("native cue tick did not publish its dwell progress");
        };
        assert!((0.3..0.7).contains(&progress));
        assert_eq!(hover_position, [5.0, 50.0]);

        niri.edge_trigger_candidate.as_mut().unwrap().armed_at =
            Some(Instant::now() - Duration::from_millis(351));
        assert!(niri.edge_trigger_cue_tick(generation, position, false));
        let Ok(niri_ipc::Event::EdgeHover { progress, .. }) = events.try_recv() else {
            panic!("completed dwell did not publish its final cue progress");
        };
        assert_eq!(progress, 1.);
        assert!(matches!(
            events.try_recv(),
            Ok(niri_ipc::Event::EdgeTriggered { .. })
        ));
        assert!(niri.edge_trigger_cue_tick(generation, position, false));
        assert!(
            events.try_recv().is_err(),
            "stationary triggered cues must not be resent"
        );

        let mut fixture = cue_configured_fixture();
        let events = attach_edge_events(&mut fixture);
        let position = left_edge_position(&fixture, 1) + Point::new(5., 0.);
        enter_left_edge(fixture.niri(), position);
        assert!(matches!(
            events.try_recv(),
            Ok(niri_ipc::Event::EdgeApproach { .. })
        ));
        let niri = fixture.niri();
        let generation = {
            let candidate = niri.edge_trigger_candidate.as_mut().unwrap();
            candidate.armed = false;
            candidate.armed_at = None;
            candidate.pressure_required = 100.;
            candidate.generation
        };
        assert!(niri.edge_trigger_cue_tick(generation, position + Point::new(1., 0.), false));
        let Ok(niri_ipc::Event::EdgeHover { progress, .. }) = events.try_recv() else {
            panic!("unarmed cue did not track pointer movement");
        };
        assert_eq!(progress, 0.);
        niri.update_edge_trigger(position + Point::new(5.1, 0.), Point::new(5.1, 0.));
        assert!(matches!(
            events.try_recv(),
            Ok(niri_ipc::Event::EdgeLeft { .. })
        ));
        assert!(
            events.try_recv().is_err(),
            "leaving before dwell must not trigger"
        );
    }

    #[test]
    fn post_trigger_cue_stops_when_overview_suppresses_the_edge() {
        let mut fixture = cue_configured_fixture();
        let events = attach_edge_events(&mut fixture);
        let position = left_edge_position(&fixture, 1) + Point::new(5., 0.);
        enter_left_edge(fixture.niri(), position);
        assert!(matches!(
            events.try_recv(),
            Ok(niri_ipc::Event::EdgeApproach { .. })
        ));
        let niri = fixture.niri();
        let generation = {
            let candidate = niri.edge_trigger_candidate.as_mut().unwrap();
            candidate.armed_at = Some(Instant::now() - Duration::from_millis(351));
            candidate.generation
        };
        if let Some(token) = niri.edge_trigger_timer.take() {
            niri.event_loop.remove(token);
        }
        assert!(niri.edge_trigger_cue_tick(generation, position, false));
        let Ok(niri_ipc::Event::EdgeHover { progress, .. }) = events.try_recv() else {
            panic!("completed dwell did not publish its final cue progress");
        };
        assert_eq!(progress, 1.);
        assert!(matches!(
            events.try_recv(),
            Ok(niri_ipc::Event::EdgeTriggered { .. })
        ));
        niri.layout.toggle_overview();
        assert!(!niri.edge_trigger_cue_tick(generation, position, false));
        assert!(matches!(
            events.try_recv(),
            Ok(niri_ipc::Event::EdgeLeft { .. })
        ));
        assert!(
            events.try_recv().is_err(),
            "suppressed cues must not publish another hover update"
        );
    }

    #[test]
    fn event_action_emits_edge_left_at_output_local_leave_position() {
        let mut fixture = configured_fixture();
        let events = attach_edge_events(&mut fixture);
        let position = left_edge_position(&fixture, 1);
        enter_left_edge(fixture.niri(), position);

        let niri = fixture.niri();
        let candidate = niri.edge_trigger_candidate.as_mut().unwrap();
        let generation = candidate.generation;
        candidate.armed_at = Some(Instant::now() - Duration::from_millis(351));
        if let Some(token) = niri.edge_trigger_timer.take() {
            niri.event_loop.remove(token);
        }
        niri.fire_edge_trigger(generation, position);

        let Ok(niri_ipc::Event::EdgeTriggered {
            output: started_output,
            edge: started_edge,
            position: started_position,
        }) = events.try_recv()
        else {
            panic!("firing an event edge did not emit EdgeTriggered");
        };
        assert_eq!(started_output, "headless-1");
        assert_eq!(started_edge, niri_ipc::Edge::Left);
        assert_eq!(started_position, [0.0, 50.0]);
        let leaving_position = position + Point::new(12., 0.);
        niri.update_edge_trigger(leaving_position, Point::new(12., 0.));
        let Ok(niri_ipc::Event::EdgeLeft {
            output,
            edge,
            position,
        }) = events.try_recv()
        else {
            panic!("leaving a fired event edge did not emit EdgeLeft");
        };
        assert_eq!(output, "headless-1");
        assert_eq!(edge, niri_ipc::Edge::Left);
        assert_eq!(position, [12.0, 50.0]);
    }

    #[test]
    fn edge_left_tracks_position_across_scaled_output_boundary() {
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
            trigger_distance: None,
            trigger_length: None,
        });
        config.outputs.0.push(niri_config::Output {
            name: String::from("headless-2"),
            scale: Some(niri_config::FloatOrInt(1.5)),
            ..Default::default()
        });
        let mut fixture = Fixture::with_config(config);
        fixture.add_output(1, (1920, 1080));
        fixture.add_output(2, (2880, 1800));
        let output = fixture.niri_output(2);
        let geometry = fixture
            .niri()
            .global_space
            .output_geometry(&output)
            .unwrap();
        assert_eq!((geometry.size.w, geometry.size.h), (1920, 1200));
        let events = attach_edge_events(&mut fixture);
        let position = left_edge_position(&fixture, 2);
        enter_left_edge(fixture.niri(), position);

        let niri = fixture.niri();
        let candidate = niri.edge_trigger_candidate.as_mut().unwrap();
        let generation = candidate.generation;
        candidate.armed_at = Some(Instant::now() - Duration::from_millis(351));
        if let Some(token) = niri.edge_trigger_timer.take() {
            niri.event_loop.remove(token);
        }
        niri.fire_edge_trigger(generation, position);
        assert!(matches!(
            events.try_recv(),
            Ok(niri_ipc::Event::EdgeTriggered { .. })
        ));

        let leaving_position = position - Point::new(10., 0.);
        niri.update_edge_trigger(leaving_position, Point::new(-10., 0.));
        let Ok(niri_ipc::Event::EdgeLeft {
            output,
            edge,
            position,
        }) = events.try_recv()
        else {
            panic!("cross-output edge leave did not emit EdgeLeft");
        };
        assert_eq!(output, "headless-2");
        assert_eq!(edge, niri_ipc::Edge::Left);
        assert_eq!(position, [-10.0, 50.0]);
    }

    #[test]
    fn unfired_and_spawn_action_candidates_do_not_emit_edge_left() {
        let mut fixture = configured_fixture();
        let events = attach_edge_events(&mut fixture);
        let position = left_edge_position(&fixture, 1);
        enter_left_edge(fixture.niri(), position);
        fixture
            .niri()
            .update_edge_trigger(position + Point::new(12., 0.), Point::new(12., 0.));
        assert!(events.try_recv().is_err());

        let mut fixture = configured_fixture_with_action(EdgeTriggerAction {
            event: false,
            spawn: Some(vec!["test-command".into()]),
            spawn_sh: None,
        });
        let events = attach_edge_events(&mut fixture);
        let position = left_edge_position(&fixture, 1);
        enter_left_edge(fixture.niri(), position);
        let niri = fixture.niri();
        niri.edge_trigger_candidate.as_mut().unwrap().fired = true;
        niri.clear_edge_trigger_at(Some(position + Point::new(12., 0.)));
        assert!(events.try_recv().is_err());
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
    fn output_reposition_and_config_reload_clear_pending_dwell() {
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

        let unrelated_output = fixture.niri_output(1);
        fixture.niri().remove_output(&unrelated_output);
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
