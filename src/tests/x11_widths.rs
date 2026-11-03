use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use niri_config::Config;
use serde_json::Value;

use super::Fixture;
use crate::layout::LayoutElement as _;

const X11_CLIENT: &str = r#"
import ctypes as c, json, sys

class Pair(c.Structure):
    _fields_ = [("x", c.c_int), ("y", c.c_int)]

class Hints(c.Structure):
    _fields_ = ([ ("flags", c.c_long) ] +
                [(n, c.c_int) for n in ("x y width height min_width min_height max_width max_height width_inc height_inc").split()] +
                [("min_aspect", Pair), ("max_aspect", Pair)] +
                [(n, c.c_int) for n in ("base_width base_height win_gravity").split()])

x = c.CDLL("libX11.so.6")
def fn(name, args, result):
    f = getattr(x, name); f.argtypes = args; f.restype = result; return f

open_display = fn("XOpenDisplay", [c.c_char_p], c.c_void_p)
root_window = fn("XDefaultRootWindow", [c.c_void_p], c.c_ulong)
create_window = fn("XCreateSimpleWindow", [c.c_void_p, c.c_ulong, c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_uint, c.c_ulong, c.c_ulong], c.c_ulong)
store_name = fn("XStoreName", [c.c_void_p, c.c_ulong, c.c_char_p], c.c_int)
intern_atom = fn("XInternAtom", [c.c_void_p, c.c_char_p, c.c_int], c.c_ulong)
change_property = fn("XChangeProperty", [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_int, c.c_int, c.c_void_p, c.c_int], c.c_int)
set_hints = fn("XSetWMNormalHints", [c.c_void_p, c.c_ulong, c.POINTER(Hints)], c.c_int)
get_hints = fn("XGetWMNormalHints", [c.c_void_p, c.c_ulong, c.POINTER(Hints), c.POINTER(c.c_long)], c.c_int)
map_window = fn("XMapWindow", [c.c_void_p, c.c_ulong], c.c_int)
sync = fn("XSync", [c.c_void_p, c.c_int], c.c_int)
get_geometry = fn("XGetGeometry", [c.c_void_p, c.c_ulong, c.POINTER(c.c_ulong), c.POINTER(c.c_int), c.POINTER(c.c_int), c.POINTER(c.c_uint), c.POINTER(c.c_uint), c.POINTER(c.c_uint), c.POINTER(c.c_uint)], c.c_int)
close_display = fn("XCloseDisplay", [c.c_void_p], c.c_int)

d = open_display(sys.argv[1].encode())
if not d: raise RuntimeError("XOpenDisplay failed")
window = create_window(d, root_window(d), 0, 0, 500, 400, 0, 0, 0)
store_name(d, window, b"Niri native width proof")
klass = c.create_string_buffer(b"niri-width-proof\0NiriWidthProof\0")
change_property(d, window, intern_atom(d, b"WM_CLASS", 0), intern_atom(d, b"STRING", 0), 8, 0, klass, len(klass.raw) - 1)
h = Hints()
h.flags = 1023
h.x, h.y, h.width, h.height = 7, 9, 480, 320
h.min_width, h.min_height = 1500, 450
h.max_width, h.max_height = 1800, 900
h.width_inc, h.height_inc = 13, 17
h.min_aspect, h.max_aspect = Pair(4, 3), Pair(16, 9)
h.base_width, h.base_height, h.win_gravity = 30, 40, 1
set_hints(d, window, c.byref(h))
map_window(d, window); sync(d, 0)
print(json.dumps({"ready": True}), flush=True)

def snapshot():
    sync(d, 0)
    root = c.c_ulong(); gx = c.c_int(); gy = c.c_int()
    width = c.c_uint(); height = c.c_uint(); border = c.c_uint(); depth = c.c_uint()
    if not get_geometry(d, window, c.byref(root), c.byref(gx), c.byref(gy), c.byref(width), c.byref(height), c.byref(border), c.byref(depth)):
        raise RuntimeError("XGetGeometry failed")
    current, supplied = Hints(), c.c_long()
    if not get_hints(d, window, c.byref(current), c.byref(supplied)):
        raise RuntimeError("XGetWMNormalHints failed")
    raw = {"flags": current.flags}
    for name, _ in Hints._fields_:
        value = getattr(current, name)
        raw[name] = {"x": value.x, "y": value.y} if isinstance(value, Pair) else value
    return {"width": width.value, "height": height.value, "hints": raw}

for command in sys.stdin:
    command = command.strip()
    if command == "query":
        print(json.dumps(snapshot()), flush=True)
    elif command == "exit":
        break
close_display(d)
"#;

struct X11Client {
    child: Child,
    input: ChildStdin,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
}

impl X11Client {
    fn start(display: &str) -> Self {
        let mut child = Command::new("python3")
            .arg("-c")
            .arg(X11_CLIENT)
            .arg(display)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start X11 proof client");
        let stdout = child.stdout.take().expect("proof client stdout");
        let (sender, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            input: child.stdin.take().expect("proof client stdin"),
            child,
            lines,
            reader: Some(reader),
        }
    }

    fn query(&mut self, fixture: &mut Fixture) -> Value {
        writeln!(self.input, "query").expect("send X11 query");
        self.input.flush().expect("flush X11 query");
        self.next_line(fixture)
    }

    fn next_line(&mut self, fixture: &mut Fixture) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            fixture.dispatch();
            match self.lines.recv_timeout(Duration::from_millis(5)) {
                Ok(line) => return serde_json::from_str(&line).expect("X11 proof JSON"),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("X11 proof client exited: {:?}", self.child.try_wait());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for X11 proof client");
            }
        }
    }

    fn wait_for_width(&mut self, fixture: &mut Fixture, predicate: impl Fn(u64) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let result = self.query(fixture);
            if predicate(result["width"].as_u64().unwrap_or_default()) {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "X11 width did not reach expected range: {result}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn shutdown(&mut self, fixture: &mut Fixture) {
        let _ = writeln!(self.input, "exit");
        let _ = self.input.flush();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            fixture.dispatch();
            if self.child.try_wait().expect("poll proof client").is_some() {
                break;
            }
            if Instant::now() >= deadline {
                // This is the exact Python child started by this test.
                let _ = self.child.kill();
                let _ = self.child.wait();
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for X11Client {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn first_window(fixture: &mut Fixture) -> Option<(smithay::desktop::Window, bool, i32)> {
    let niri = fixture.niri();
    let mut result = None;
    niri.layout.with_windows(|mapped, _, _, _| {
        if result.is_none() {
            result = Some((
                mapped.window.clone(),
                mapped.is_floating(),
                mapped.min_size().w,
            ));
        }
    });
    result
}

#[test]
#[ignore = "requires Xwayland, libX11, Python, and NIRI_TEST_XWAYLAND_SATELLITE"]
fn managed_x11_minimum_width_policy_reaches_real_x11_client() {
    let Some(satellite_path) = std::env::var_os("NIRI_TEST_XWAYLAND_SATELLITE") else {
        eprintln!("skipping: NIRI_TEST_XWAYLAND_SATELLITE is unset");
        return;
    };
    assert!(Path::new(&satellite_path).is_absolute());
    let runtime =
        std::env::var_os("XDG_RUNTIME_DIR").expect("isolated XDG_RUNTIME_DIR is required");
    assert!(Path::new(&runtime).is_dir());

    let make_config = |enabled| {
        let mut config = Config::default();
        config.xwayland_satellite.path = satellite_path.to_string_lossy().into_owned();
        config.nacre.proportional_x11_widths = enabled;
        config.outputs = niri_config::Outputs(vec![niri_config::Output {
            name: "headless-1".to_owned(),
            scale: Some(niri_config::FloatOrInt(1.5)),
            ..Default::default()
        }]);
        config
    };

    let mut fixture = Fixture::with_config(make_config(true));
    fixture.add_output(1, (1920, 1080));
    crate::utils::xwayland::satellite::setup(fixture.niri_state());
    let display = fixture
        .niri()
        .satellite
        .as_ref()
        .expect("managed satellite should initialize")
        .display_name()
        .to_owned();

    let mut client = X11Client::start(&display);
    let ready = client.next_line(&mut fixture);
    assert_eq!(ready["ready"], true);

    let mapped_deadline = Instant::now() + Duration::from_secs(15);
    let (window, floating, min_width) = loop {
        fixture.dispatch();
        if let Some(window) = first_window(&mut fixture) {
            break window;
        }
        assert!(
            Instant::now() < mapped_deadline,
            "X11 toplevel did not map through the managed satellite"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert!(!floating);
    assert_eq!(
        min_width, 1,
        "native tiled policy should relax only the effective minimum"
    );

    let tiled = client.wait_for_width(&mut fixture, |width| width < 1500);
    assert!(tiled["width"].as_u64().unwrap() < 1500);
    let original_hints = tiled["hints"].clone();
    assert_eq!(original_hints["min_width"], 1500);
    assert_eq!(original_hints["max_width"], 1800);
    assert_eq!(original_hints["width_inc"], 13);
    assert_eq!(original_hints["height_inc"], 17);
    assert_eq!(
        original_hints["min_aspect"],
        serde_json::json!({"x": 4, "y": 3})
    );
    assert_eq!(
        original_hints["max_aspect"],
        serde_json::json!({"x": 16, "y": 9})
    );

    fixture
        .niri()
        .layout
        .set_window_width(Some(&window), niri_ipc::SizeChange::SetProportion(100.0));
    let maximum_geometry = client.wait_for_width(&mut fixture, |width| width >= 1500);
    let maximum_width = maximum_geometry["width"].as_u64().unwrap();
    assert!(
        maximum_width <= 1800,
        "X11 maximum width was exceeded: {maximum_width}"
    );
    assert_eq!(maximum_geometry["hints"], original_hints);

    fixture
        .niri()
        .layout
        .set_window_floating(Some(&window), true);
    let floating_geometry = client.wait_for_width(&mut fixture, |width| width >= 1500);
    assert!(floating_geometry["width"].as_u64().unwrap() >= 1500);
    assert_eq!(floating_geometry["hints"], original_hints);
    let (is_floating, min_width) = {
        let niri = fixture.niri();
        let mut result = None;
        niri.layout.with_windows(|mapped, _, _, _| {
            result = Some((mapped.is_floating(), mapped.min_size().w));
        });
        result.unwrap()
    };
    assert!(is_floating);
    assert_eq!(min_width, 1000);

    fixture
        .niri()
        .layout
        .set_window_floating(Some(&window), false);
    fixture
        .niri()
        .layout
        .set_window_width(Some(&window), niri_ipc::SizeChange::SetProportion(50.0));
    client.wait_for_width(&mut fixture, |width| width < 1500);
    fixture.niri_state().reload_config(Ok(make_config(false)));
    let disabled_geometry = client.wait_for_width(&mut fixture, |width| width >= 1500);
    assert!(disabled_geometry["width"].as_u64().unwrap() >= 1500);
    assert_eq!(disabled_geometry["hints"], original_hints);

    client.shutdown(&mut fixture);
}
