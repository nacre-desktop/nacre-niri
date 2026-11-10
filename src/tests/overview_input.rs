//! Input routing through the rendered collection overview.

use niri_config::Config;
use smithay::desktop::Window;
use smithay::utils::{Logical, Point};
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::Fixture;

fn fixture() -> Fixture {
    let config = Config::parse_mem(
        "output \"headless-1\" { scale 1.5; }\noutput \"headless-2\" { scale 1.0; }\ninput { focus-follows-mouse max-scroll-amount=\"0%\"; }",
    )
    .unwrap();
    let mut config = config;
    config.layout.collections.enabled = true;
    let mut fixture = Fixture::with_config(config);
    fixture.add_output(1, (2880, 1800));
    fixture.add_output(2, (3440, 1440));
    fixture
}

fn set_time(niri: &mut crate::niri::Niri, time: std::time::Duration) {
    let now = niri.clock.now();
    niri.clock.set_unadjusted(now);
    let _ = niri.clock.now();
    niri.clock.set_unadjusted(std::time::Duration::ZERO);
    niri.clock.set_rate(1.0);
    let _ = niri.clock.now();
    niri.clock.set_unadjusted(time);
    let _ = niri.clock.now();
    niri.clock.set_rate(0.0);
}

fn create_window(fixture: &mut Fixture, client: ClientId) -> WlSurface {
    let surface = {
        let window = fixture.client(client).create_window();
        let surface = window.surface.clone();
        window.commit();
        surface
    };
    fixture.roundtrip(client);
    let window = fixture.client(client).window(&surface);
    window.attach_new_buffer();
    window.set_size(900, 650);
    window.ack_last_and_commit();
    fixture.double_roundtrip(client);
    surface
}

fn populated_fixture() -> (Fixture, ClientId) {
    let mut fixture = fixture();
    let client = fixture.add_client();

    // Each output has its own shown desk and window.
    create_window(&mut fixture, client);
    create_window(&mut fixture, client);
    let output2 = fixture.niri_output(2);
    fixture.niri().layout.focus_output(&output2);
    create_window(&mut fixture, client);
    create_window(&mut fixture, client);

    // Add a detached desk with a window so both outputs show a side-column preview.
    let output1 = fixture.niri_output(1);
    fixture.niri().layout.focus_output(&output1);
    let output1_collection = fixture
        .niri()
        .layout
        .collections()
        .into_iter()
        .find(|collection| collection.output.as_deref() == Some("headless-1"))
        .expect("output one collection")
        .id;
    fixture.niri().layout.create_collection(None);
    create_window(&mut fixture, client);
    fixture
        .niri()
        .layout
        .focus_collection(&niri_config::WorkspaceReference::Id(output1_collection));
    fixture.niri().layout.focus_output(&output1);
    fixture.niri().layout.open_overview();
    fixture.niri_complete_animations();

    (fixture, client)
}

fn target_window(fixture: &Fixture, output_idx: u8, hidden: bool) -> Window {
    let niri = &fixture.state.server.state.niri;
    let output = fixture.niri_output(output_idx);
    let focused = niri.layout.focus().map(|window| window.window.clone());
    niri.layout
        .windows()
        .find_map(|(monitor, window)| {
            let belongs = if hidden {
                monitor.is_none()
            } else {
                monitor.is_some_and(|monitor| monitor.output() == &output)
            };
            (belongs && (hidden || Some(&window.window) != focused.as_ref()))
                .then_some(window.window.clone())
        })
        .expect(if hidden {
            "detached window"
        } else {
            "shown window"
        })
}

fn hit_point(fixture: &mut Fixture, output_idx: u8, expected: &Window) -> Point<f64, Logical> {
    let output = fixture.niri_output(output_idx);
    let scale = output.current_scale().fractional_scale();
    let mode = output.current_mode().unwrap().size;
    let size = (mode.w as f64 / scale, mode.h as f64 / scale);
    let origin = fixture
        .niri()
        .global_space
        .output_geometry(&output)
        .unwrap()
        .loc
        .to_f64();

    for y in (4..size.1 as i32).step_by(8) {
        for x in (4..size.0 as i32).step_by(8) {
            let point = origin + Point::from((x as f64, y as f64));
            if fixture
                .niri()
                .contents_under(point)
                .window
                .is_some_and(|(window, _)| &window == expected)
            {
                return point;
            }
        }
    }

    panic!("overview window had no input hit on output {output_idx}");
}

fn move_pointer_to(
    fixture: &mut Fixture,
    client: ClientId,
    output_idx: u8,
    point: Point<f64, Logical>,
) {
    let output = fixture.niri_output(output_idx);
    let origin = fixture
        .niri()
        .global_space
        .output_geometry(&output)
        .unwrap()
        .loc
        .to_f64();
    let scale = output.current_scale().fractional_scale();
    let mode = output.current_mode().unwrap().size;
    let local = point - origin;
    let pointer = fixture
        .client(client)
        .create_virtual_pointer(if output_idx == 1 {
            "headless-1"
        } else {
            "headless-2"
        });
    pointer.motion_absolute(
        1,
        (local.x * scale).round() as u32,
        (local.y * scale).round() as u32,
        mode.w as u32,
        mode.h as u32,
    );
    pointer.frame();
    fixture.double_roundtrip(client);
}

#[test]
fn overview_pointer_motion_hits_shown_and_side_windows_on_both_output_scales() {
    let (mut fixture, client) = populated_fixture();
    for output_idx in [1, 2] {
        for hidden in [false, true] {
            let expected = target_window(&fixture, output_idx, hidden);
            let point = hit_point(&mut fixture, output_idx, &expected);
            move_pointer_to(&mut fixture, client, output_idx, point);
            assert_eq!(
                fixture
                    .niri()
                    .window_under_cursor()
                    .map(|window| window.window.clone()),
                Some(expected.clone()),
                "hover hit on output {output_idx}, hidden={hidden}"
            );
            if !hidden {
                assert_eq!(fixture.niri().layout.focus().unwrap().window, expected);
            }
        }
    }
}

#[test]
fn overview_click_on_shown_and_side_windows_selects_and_closes_overview() {
    for output_idx in [1, 2] {
        for hidden in [false, true] {
            let (mut fixture, client) = populated_fixture();
            let expected = target_window(&fixture, output_idx, hidden);
            let point = hit_point(&mut fixture, output_idx, &expected);
            move_pointer_to(&mut fixture, client, output_idx, point);
            let pointer = fixture
                .client(client)
                .create_virtual_pointer(if output_idx == 1 {
                    "headless-1"
                } else {
                    "headless-2"
                });
            pointer.button(2, 0x110, wl_pointer::ButtonState::Pressed);
            pointer.frame();
            fixture.double_roundtrip(client);
            pointer.button(3, 0x110, wl_pointer::ButtonState::Released);
            pointer.frame();
            fixture.double_roundtrip(client);

            assert!(!fixture.niri().layout.is_overview_open());
            assert_eq!(fixture.niri().layout.focus().unwrap().window, expected);
        }
    }
}

#[test]
fn overview_wheel_switch_survives_previous_row_hover() {
    use std::time::Duration;

    for down in [true, false] {
        let mut fixture = fixture();
        let client = fixture.add_client();
        let output = fixture.niri_output(1);
        fixture.niri().layout.focus_output(&output);

        create_window(&mut fixture, client);
        fixture.niri().layout.switch_workspace_down();
        create_window(&mut fixture, client);
        let middle_window = fixture.niri().layout.focus().unwrap().window.clone();
        let middle_workspace = fixture
            .niri()
            .layout
            .active_workspace()
            .unwrap()
            .id()
            .clone();
        fixture.niri().layout.switch_workspace_down();
        create_window(&mut fixture, client);
        let bottom_window = fixture.niri().layout.focus().unwrap().window.clone();
        let bottom_workspace = fixture
            .niri()
            .layout
            .active_workspace()
            .unwrap()
            .id()
            .clone();

        if down {
            fixture.niri().layout.switch_workspace_up();
            fixture.niri_complete_animations();
        }
        fixture.niri().layout.open_overview();
        fixture.niri_complete_animations();
        set_time(fixture.niri(), Duration::ZERO);

        let target_window = if down {
            bottom_window.clone()
        } else {
            middle_window.clone()
        };
        let target_workspace = if down {
            bottom_workspace
        } else {
            middle_workspace
        };
        let previous_window = if down { middle_window } else { bottom_window };

        let mode = output.current_mode().unwrap().size;
        let pointer = fixture.client(client).create_virtual_pointer("headless-1");
        pointer.motion_absolute(
            1,
            mode.w as u32 / 2,
            mode.h as u32 / 2,
            mode.w as u32,
            mode.h as u32,
        );
        pointer.frame();
        fixture.double_roundtrip(client);

        let axis = wl_pointer::Axis::VerticalScroll;
        fixture.state.server.dispatch();
        assert!(fixture.niri().bind_cooldown_timers.is_empty());
        pointer.axis_discrete(
            2,
            axis,
            if down { 15.0 } else { -15.0 },
            if down { 1 } else { -1 },
        );
        pointer.axis_source(wl_pointer::AxisSource::Wheel);
        pointer.frame();
        fixture.client(client).connection.flush().unwrap();
        fixture.roundtrip(client);
        set_time(fixture.niri(), Duration::from_millis(100));
        fixture.niri().advance_animations();
        assert_eq!(
            fixture.niri().layout.active_workspace().unwrap().id(),
            target_workspace,
            "wheel must select its target row"
        );

        let previous_point = hit_point(&mut fixture, 1, &previous_window);
        move_pointer_to(&mut fixture, client, 1, previous_point);
        assert_eq!(
            fixture
                .niri()
                .window_under_cursor()
                .map(|window| window.window.clone()),
            Some(previous_window.clone()),
            "pointer must hover the previous row"
        );
        assert_eq!(
            fixture.niri().layout.active_workspace().unwrap().id(),
            target_workspace,
            "hover over the row before the wheel must not retarget"
        );
        assert!(fixture.niri().layout.are_animations_ongoing(Some(&output)));

        let target_point = hit_point(&mut fixture, 1, &target_window);
        move_pointer_to(&mut fixture, client, 1, target_point);
        assert_eq!(
            fixture.niri().layout.active_workspace().unwrap().id(),
            target_workspace,
            "same-row hover must preserve the wheel target"
        );
        assert!(fixture.niri().layout.are_animations_ongoing(Some(&output)));

        set_time(fixture.niri(), Duration::from_secs(5));
        fixture.niri().advance_animations();
        assert!(!fixture.niri().layout.are_animations_ongoing(Some(&output)));
        assert_eq!(
            fixture.niri().layout.active_workspace().unwrap().id(),
            target_workspace,
            "wheel target must remain selected after the animation completes"
        );
    }
}
