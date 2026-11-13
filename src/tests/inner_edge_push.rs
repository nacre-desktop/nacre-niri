//! NN0041: sticky inner monitor edges and output adjacency over IPC.

use std::time::{Duration, Instant};

use niri_config::{Config, EdgeTrigger, EdgeTriggerAction};
use niri_ipc::{Edge, Event};
use smithay::utils::{Logical, Point};

use super::Fixture;

fn event_trigger() -> EdgeTrigger {
    EdgeTrigger {
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
    }
}

/// Two 1920x1080 outputs side by side, with event triggers on the left and right edges.
fn side_by_side(config: Config) -> Fixture {
    let mut config = config;
    config.edge_triggers.left = Some(event_trigger());
    config.edge_triggers.right = Some(event_trigger());
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (1920, 1080));
    fixture.add_output(2, (1920, 1080));
    fixture
}

/// Pointer on the last column of the left output, level with the neighbour.
fn at_edge() -> Point<f64, Logical> {
    Point::new(1919., 500.)
}

#[test]
fn slow_push_is_held_and_fires_after_the_hold_time() {
    let mut fixture = side_by_side(Config::default());
    let server = crate::ipc::server::IpcServer::start(&fixture.niri().event_loop, None).unwrap();
    let events = server.subscribe_edge_events();
    fixture.niri().ipc_server = Some(server);
    let niri = fixture.niri();

    // 3 px per 10 ms = 300 px/s: a deliberate lean, well under the fling speed.
    let delta = Point::new(3., 0.);
    let edge = at_edge();
    let mut held = edge;
    for step in 0..6u64 {
        held = niri.sticky_inner_edge(held, held + delta, delta, step * 10_000);
        assert_eq!(held, edge, "step {step}");
        niri.update_edge_trigger(held, delta);
    }
    let candidate = niri.edge_trigger_candidate.as_mut().unwrap();
    assert_eq!(candidate.output, "headless-1");
    assert_eq!(candidate.edge, Edge::Right);
    assert_eq!(candidate.dwell_time, Duration::from_millis(180));
    assert!(candidate.armed);
    let generation = candidate.generation;
    candidate.armed_at = Some(Instant::now() - Duration::from_millis(181));
    if let Some(token) = niri.edge_trigger_timer.take() {
        niri.event_loop.remove(token);
    }
    niri.fire_edge_trigger(generation, held);
    match events.try_recv() {
        Ok(Event::EdgeTriggered {
            output,
            edge,
            position,
        }) => {
            assert_eq!(output, "headless-1");
            assert_eq!(edge, Edge::Right);
            assert_eq!(position, [1919., 500.]);
        }
        other => panic!("expected EdgeTriggered, got {other:?}"),
    }

    // A sustained shove after the trigger still crosses; leaving emits EdgeLeft as usual.
    let shove = Point::new(40., 0.);
    let mut pos = held;
    let mut crossed = false;
    for step in 6..12u64 {
        let next = niri.sticky_inner_edge(pos, pos + shove, shove, step * 10_000);
        niri.update_edge_trigger(next, shove);
        if next != pos {
            crossed = true;
            break;
        }
        pos = next;
    }
    assert!(crossed, "a 4000 px/s shove must cross");
    assert!(matches!(events.try_recv(), Ok(Event::EdgeLeft { .. })));
}

#[test]
fn fling_crosses_at_once_and_accumulated_push_crosses_without_a_fling() {
    let mut fixture = side_by_side(Config::default());
    let niri = fixture.niri();

    // First event at the edge is held (no velocity history yet)...
    let fling = Point::new(2., 0.);
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), at_edge() + fling, fling, 0),
        at_edge()
    );
    // ...the next one, 2 px in 1 ms = 2000 px/s, is a fling and passes through.
    let after = at_edge() + fling;
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), after, fling, 1_000),
        after
    );
    assert!(niri.inner_edge_push.is_none());

    // 30 px per 50 ms = 600 px/s: no fling, but 64 px of push cross on the third event
    // (30, then 30 + 30 - 12.5 of decay = 47.5, then 47.5 + 30 - 12.5 = 65).
    let shove = Point::new(30., 0.);
    let after = at_edge() + shove;
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), after, shove, 1_000_000),
        at_edge()
    );
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), after, shove, 1_050_000),
        at_edge()
    );
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), after, shove, 1_100_000),
        after
    );

    // Decay: a lean that stops for a second starts over.
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), after, shove, 2_000_000),
        at_edge()
    );
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), after, shove, 2_050_000),
        at_edge()
    );
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), after, shove, 3_050_000),
        at_edge()
    );
}

#[test]
fn drift_along_the_edge_outer_edges_and_disabled_edges_pass_through() {
    let mut fixture = side_by_side(Config::default());
    let niri = fixture.niri();

    // Moving along the inner edge stays on the output and keeps no push state.
    let along = Point::new(0., 5.);
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), at_edge() + along, along, 0),
        at_edge() + along
    );
    assert!(niri.inner_edge_push.is_none());

    // The outer left edge has a trigger too, but nothing lies across it: the ordinary clip
    // applies, not the hold.
    let out = Point::new(-5., 0.);
    let at_left = Point::new(0., 500.);
    assert_eq!(
        niri.sticky_inner_edge(at_left, at_left + out, out, 0),
        at_left + out
    );

    // Diagonal exit through the corner into the neighbour clamps both axes.
    let corner = Point::new(1919., 1079.);
    let diagonal = Point::new(4., 3.);
    assert_eq!(
        niri.sticky_inner_edge(corner, corner + diagonal, diagonal, 0),
        corner
    );

    // Per-edge disable and the global off switch let the pointer cross.
    let push = Point::new(3., 0.);
    for off in [false, true] {
        let mut config = Config::default();
        config.edge_triggers.inner_edge_push.off = off;
        config.edge_triggers.inner_edge_push.right = off;
        let mut fixture = side_by_side(config);
        let niri = fixture.niri();
        assert_eq!(
            niri.sticky_inner_edge(at_edge(), at_edge() + push, push, 0),
            at_edge() + push
        );
    }

    // An inner edge without a configured trigger is not sticky either.
    let mut fixture = Fixture::with_config(Config::default());
    fixture.add_output(1, (1920, 1080));
    fixture.add_output(2, (1920, 1080));
    let niri = fixture.niri();
    assert_eq!(
        niri.sticky_inner_edge(at_edge(), at_edge() + push, push, 0),
        at_edge() + push
    );
}

#[test]
fn output_neighbours_reach_nacre_streams_on_hotplug_only() {
    let mut fixture = Fixture::new();
    let server = crate::ipc::server::IpcServer::start(&fixture.niri().event_loop, None).unwrap();
    let nacre = server.subscribe_collection_events();
    let standard = server.subscribe_standard_events();
    fixture.niri().ipc_server = Some(server);

    fixture.add_output(1, (1920, 1080));
    fixture.niri_state().refresh_ipc_outputs();
    // One output: the list is still empty, so nothing is sent.
    assert!(nacre.try_recv().is_err());

    fixture.add_output(2, (1920, 1080));
    fixture.niri_state().refresh_ipc_outputs();
    fixture.niri_state().refresh_ipc_outputs();
    let Ok(Event::OutputNeighboursChanged { neighbours }) = nacre.try_recv() else {
        panic!("hotplug did not emit OutputNeighboursChanged");
    };
    assert_eq!(neighbours.len(), 2);
    assert_eq!(neighbours[0].output, "headless-1");
    assert_eq!(neighbours[0].edge, Edge::Right);
    assert_eq!(neighbours[0].neighbour, "headless-2");
    assert_eq!((neighbours[0].start, neighbours[0].end), (0., 1080.));
    assert_eq!(neighbours[1].edge, Edge::Left);
    assert!(nacre.try_recv().is_err());
    assert!(standard.try_recv().is_err());

    let output = fixture.niri_output(2);
    fixture.niri().remove_output(&output);
    fixture.niri_state().refresh_ipc_outputs();
    assert!(matches!(
        nacre.try_recv(),
        Ok(Event::OutputNeighboursChanged { neighbours }) if neighbours.is_empty()
    ));
}
