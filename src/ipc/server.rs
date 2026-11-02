use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::{env, io, process};

use anyhow::Context;
use async_channel::{Receiver, Sender, TrySendError};
use calloop::futures::Scheduler;
use calloop::io::Async;
use directories::BaseDirs;
use futures_util::io::{AsyncReadExt, BufReader};
use futures_util::{select_biased, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, FutureExt as _};
use niri_config::OutputName;
use niri_ipc::state::{EventStreamState, EventStreamStatePart as _};
use niri_ipc::{
    Action, Event, KeyboardLayouts, OutputConfigChanged, Overview, Reply, Request, Response,
    Timestamp, WindowLayout, Workspace,
};
use smithay::desktop::layer_map_for_output;
use smithay::input::pointer::{
    CursorIcon, CursorImageStatus, Focus, GrabStartData as PointerGrabStartData,
};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::rustix::fs::unlink;
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer};

use crate::backend::IpcOutputMap;
use crate::input::pick_window_grab::PickWindowGrab;
use crate::layout::workspace::WorkspaceId;
use crate::niri::State;
use crate::utils::{version, with_toplevel_role};
use crate::window::Mapped;

// If an event stream client fails to read events fast enough that we accumulate more than this
// number in our buffer, we drop that event stream client.
const EVENT_STREAM_BUFFER_SIZE: usize = 64;

pub struct IpcServer {
    /// Path to the IPC socket.
    ///
    /// This is `None` when creating `IpcServer` without a socket.
    pub socket_path: Option<PathBuf>,
    event_streams: Rc<RefCell<Vec<EventStreamSender>>>,
    event_stream_state: Rc<RefCell<EventStreamState>>,
    overview_hover: Rc<Cell<Option<u64>>>,
}

struct ClientCtx {
    event_loop: LoopHandle<'static, State>,
    scheduler: Scheduler<()>,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    event_streams: Rc<RefCell<Vec<EventStreamSender>>>,
    event_stream_state: Rc<RefCell<EventStreamState>>,
    overview_hover: Rc<Cell<Option<u64>>>,
}

struct EventStreamClient {
    events: Receiver<Event>,
    disconnect: Receiver<()>,
    write: Box<dyn AsyncWrite + Unpin>,
}

struct EventStreamSender {
    overview_hover: bool,
    events: Sender<Event>,
    disconnect: Sender<()>,
    include_collections: bool,
}

impl IpcServer {
    pub fn start(
        event_loop: &LoopHandle<'static, State>,
        wayland_socket_name: Option<&OsStr>,
    ) -> anyhow::Result<Self> {
        let _span = tracy_client::span!("Ipc::start");

        let socket_path = if let Some(wayland_socket_name) = wayland_socket_name {
            let wayland_socket_name = wayland_socket_name.to_string_lossy();
            let socket_name = format!("niri.{wayland_socket_name}.{}.sock", process::id());
            let mut socket_path = socket_dir();
            socket_path.push(socket_name);

            let listener = UnixListener::bind(&socket_path).context("error binding socket")?;
            listener
                .set_nonblocking(true)
                .context("error setting socket to non-blocking")?;

            let source = Generic::new(listener, Interest::READ, Mode::Level);
            event_loop
                .insert_source(source, |_, socket, state| {
                    match socket.accept() {
                        Ok((stream, _)) => on_new_ipc_client(state, stream),
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                        Err(e) => return Err(e),
                    }

                    Ok(PostAction::Continue)
                })
                .unwrap();

            Some(socket_path)
        } else {
            None
        };

        Ok(Self {
            socket_path,
            event_streams: Rc::new(RefCell::new(Vec::new())),
            event_stream_state: Rc::new(RefCell::new(EventStreamState::default())),
            overview_hover: Rc::new(Cell::new(None)),
        })
    }

    fn send_event(&self, event: Event) {
        let mut streams = self.event_streams.borrow_mut();
        let mut to_remove = Vec::new();
        for (idx, stream) in streams.iter_mut().enumerate() {
            if (matches!(event, Event::NacreOverviewHoverChanged { .. }) && !stream.overview_hover)
                || !stream_event_allowed(&event, stream.include_collections)
            {
                continue;
            }
            match stream.events.try_send(event.clone()) {
                Ok(()) => (),
                Err(TrySendError::Closed(_)) => to_remove.push(idx),
                Err(TrySendError::Full(_)) => {
                    warn!(
                        "disconnecting IPC event stream client \
                         because it is reading events too slowly"
                    );
                    to_remove.push(idx);
                }
            }
        }

        for idx in to_remove.into_iter().rev() {
            let stream = streams.swap_remove(idx);
            let _ = stream.disconnect.send_blocking(());
        }
    }

    #[cfg(test)]
    pub(crate) fn workspace_snapshot(&self) -> Vec<Workspace> {
        self.event_stream_state
            .borrow()
            .workspaces
            .workspaces
            .values()
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn window_snapshot(&self) -> Vec<niri_ipc::Window> {
        self.event_stream_state
            .borrow()
            .windows
            .windows
            .values()
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn subscribe_standard_events(&self) -> Receiver<Event> {
        let (events, receiver) = async_channel::bounded(64);
        let (disconnect, _disconnect_receiver) = async_channel::bounded(1);
        self.event_streams.borrow_mut().push(EventStreamSender {
            overview_hover: false,
            events,
            disconnect,
            include_collections: false,
        });
        receiver
    }

    pub fn has_overview_hover_clients(&self) -> bool {
        self.event_streams
            .borrow()
            .iter()
            .any(|stream| stream.overview_hover)
    }

    pub fn update_overview_hover(&self, id: Option<u64>) {
        if self.overview_hover.replace(id) != id {
            self.send_event(Event::NacreOverviewHoverChanged { id });
        }
    }
}

fn stream_event_allowed(event: &Event, include_collections: bool) -> bool {
    include_collections || !matches!(event, Event::CollectionsChanged { .. })
}

fn event_stream_snapshot(state: &EventStreamState, include_collections: bool) -> Vec<Event> {
    state
        .replicate()
        .into_iter()
        .filter(|event| stream_event_allowed(event, include_collections))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use niri_ipc::state::EventStreamState;
    use niri_ipc::Event;

    use super::{event_stream_snapshot, IpcServer};

    #[test]
    fn default_event_stream_snapshot_keeps_collection_events_out() {
        let state = EventStreamState::default();
        let legacy = event_stream_snapshot(&state, false);
        let extension = event_stream_snapshot(&state, true);
        assert!(legacy
            .iter()
            .any(|event| matches!(event, Event::WorkspacesChanged { .. })));
        assert!(!legacy
            .iter()
            .any(|event| matches!(event, Event::CollectionsChanged { .. })));
        assert!(extension
            .iter()
            .any(|event| matches!(event, Event::CollectionsChanged { .. })));
        assert_eq!(extension.len(), legacy.len() + 1);
    }

    #[test]
    fn broadcasts_gate_collection_events_per_client() {
        let (legacy_tx, legacy_rx) = async_channel::bounded(4);
        let (extension_tx, extension_rx) = async_channel::bounded(4);
        let (hover_tx, hover_rx) = async_channel::bounded(4);
        let (legacy_disconnect, _) = async_channel::bounded(1);
        let (extension_disconnect, _) = async_channel::bounded(1);
        let (hover_disconnect, _) = async_channel::bounded(1);
        let server = IpcServer {
            socket_path: None,
            event_streams: Rc::new(RefCell::new(vec![
                super::EventStreamSender {
                    overview_hover: false,
                    events: legacy_tx,
                    disconnect: legacy_disconnect,
                    include_collections: false,
                },
                super::EventStreamSender {
                    overview_hover: false,
                    events: extension_tx,
                    disconnect: extension_disconnect,
                    include_collections: true,
                },
                super::EventStreamSender {
                    overview_hover: true,
                    events: hover_tx,
                    disconnect: hover_disconnect,
                    include_collections: false,
                },
            ])),
            event_stream_state: Rc::new(RefCell::new(EventStreamState::default())),
            overview_hover: Rc::new(Cell::new(None)),
        };
        server.send_event(Event::CollectionsChanged {
            collections: vec![],
        });
        server.update_overview_hover(Some(7));
        server.send_event(Event::OverviewOpenedOrClosed { is_open: true });

        assert!(matches!(
            legacy_rx.try_recv(),
            Ok(Event::OverviewOpenedOrClosed { is_open: true })
        ));
        assert!(legacy_rx.try_recv().is_err());
        assert!(matches!(
            extension_rx.try_recv(),
            Ok(Event::CollectionsChanged { .. })
        ));
        assert!(matches!(
            extension_rx.try_recv(),
            Ok(Event::OverviewOpenedOrClosed { is_open: true })
        ));
        assert!(matches!(
            hover_rx.try_recv(),
            Ok(Event::NacreOverviewHoverChanged { id: Some(7) })
        ));
        assert!(matches!(
            hover_rx.try_recv(),
            Ok(Event::OverviewOpenedOrClosed { is_open: true })
        ));
        assert!(hover_rx.try_recv().is_err());
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if let Some(socket_path) = &self.socket_path {
            let _ = unlink(socket_path);
        }
    }
}

fn socket_dir() -> PathBuf {
    BaseDirs::new()
        .as_ref()
        .and_then(|x| x.runtime_dir())
        .map(|x| x.to_owned())
        .unwrap_or_else(env::temp_dir)
}

fn on_new_ipc_client(state: &mut State, stream: UnixStream) {
    let _span = tracy_client::span!("on_new_ipc_client");
    trace!("new IPC client connected");

    let stream = match state.niri.event_loop.adapt_io(stream) {
        Ok(stream) => stream,
        Err(err) => {
            warn!("error making IPC stream async: {err:?}");
            return;
        }
    };

    let ipc_server = state.niri.ipc_server.as_ref().unwrap();

    let ctx = ClientCtx {
        event_loop: state.niri.event_loop.clone(),
        scheduler: state.niri.scheduler.clone(),
        ipc_outputs: state.backend.ipc_outputs(),
        event_streams: ipc_server.event_streams.clone(),
        event_stream_state: ipc_server.event_stream_state.clone(),
        overview_hover: ipc_server.overview_hover.clone(),
    };

    let future = async move {
        if let Err(err) = handle_client(ctx, stream).await {
            warn!("error handling IPC client: {err:?}");
        }
    };
    if let Err(err) = state.niri.scheduler.schedule(future) {
        warn!("error scheduling IPC stream future: {err:?}");
    }
}

async fn handle_client(ctx: ClientCtx, stream: Async<'static, UnixStream>) -> anyhow::Result<()> {
    let (read, mut write) = stream.split();
    let mut read = BufReader::new(read);

    loop {
        // Don't keep buf around to avoid clients wasting RAM by filling it with bogus data.
        let mut buf = Vec::new();
        let res = read.read_until(b'\n', &mut buf).await;
        match res {
            Ok(0) => return Ok(()),
            Ok(_) => (),
            // Normal client disconnection.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(err) => {
                return Err(err).context("error reading request");
            }
        }

        let request = serde_json::from_slice(&buf)
            .context("error parsing request")
            .map_err(|err| err.to_string());
        let requested_error = matches!(request, Ok(Request::ReturnError));
        let requested_hover = matches!(request, Ok(Request::NacreOverviewHoverStream));
        let requested_event_stream = matches!(
            request,
            Ok(Request::EventStream
                | Request::EventStreamWithCollections
                | Request::NacreOverviewHoverStream)
        );
        let include_collections = matches!(request, Ok(Request::EventStreamWithCollections));

        let reply = match request {
            Ok(request) => process(&ctx, request).await,
            Err(err) => Err(err),
        };

        if let Err(err) = &reply {
            if !requested_error {
                warn!("error processing IPC request: {err:?}");
            }
        }

        buf.clear();
        serde_json::to_writer(&mut buf, &reply).context("error formatting reply")?;
        buf.push(b'\n');
        write.write_all(&buf).await.context("error writing reply")?;

        if requested_event_stream && reply.is_ok() {
            let (events_tx, events_rx) = async_channel::bounded(EVENT_STREAM_BUFFER_SIZE);
            let (disconnect_tx, disconnect_rx) = async_channel::bounded(1);

            // Spawn a task for the client.
            let client = EventStreamClient {
                events: events_rx,
                disconnect: disconnect_rx,
                write: Box::new(write) as _,
            };
            let future = async move {
                if let Err(err) = handle_event_stream_client(client).await {
                    warn!("error handling IPC event stream client: {err:?}");
                }
            };
            if let Err(err) = ctx.scheduler.schedule(future) {
                warn!("error scheduling IPC event stream future: {err:?}");
            }

            // Send the initial state.
            {
                let state = ctx.event_stream_state.borrow();
                for event in event_stream_snapshot(&state, include_collections) {
                    events_tx
                        .try_send(event)
                        .expect("initial event burst had more events than buffer size");
                }
            }

            // Add it to the list.
            {
                let mut streams = ctx.event_streams.borrow_mut();
                if requested_hover {
                    events_tx
                        .try_send(Event::NacreOverviewHoverChanged {
                            id: ctx.overview_hover.get(),
                        })
                        .expect("initial hover event exceeded buffer");
                }
                let sender = EventStreamSender {
                    overview_hover: requested_hover,
                    events: events_tx,
                    disconnect: disconnect_tx,
                    include_collections,
                };
                streams.push(sender);
            }

            return Ok(());
        }
    }
}

async fn process(ctx: &ClientCtx, request: Request) -> Reply {
    let response = match request {
        Request::ReturnError => return Err(String::from("example compositor error")),
        Request::Version => Response::Version(version()),
        Request::NacreCapabilities => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let config = state.niri.config.borrow();
                let _ = tx.send_blocking(niri_ipc::NacreCapabilities {
                    workspace_collections: config.layout.collections.enabled,
                    proportional_x11_widths: false,
                    overview_hover: config.nacre.overview_hover,
                });
            });
            let capabilities = rx
                .recv()
                .await
                .map_err(|_| String::from("error getting Nacre capabilities"))?;
            Response::NacreCapabilities(capabilities)
        }
        Request::Outputs => {
            let ipc_outputs = ctx.ipc_outputs.lock().unwrap().clone();
            let outputs = ipc_outputs.values().cloned().map(|o| (o.name.clone(), o));
            Response::Outputs(outputs.collect())
        }
        Request::Workspaces => {
            let state = ctx.event_stream_state.borrow();
            let workspaces = state.workspaces.workspaces.values().cloned().collect();
            Response::Workspaces(workspaces)
        }
        Request::Collections => {
            let state = ctx.event_stream_state.borrow();
            Response::Collections(state.collections.collections.clone())
        }
        Request::Windows => {
            let state = ctx.event_stream_state.borrow();
            let windows = state.windows.windows.values().cloned().collect();
            Response::Windows(windows)
        }
        Request::Layers => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let mut layers = Vec::new();
                for output in state.niri.global_space.outputs() {
                    let name = output.name();
                    for surface in layer_map_for_output(output).layers() {
                        let layer = match surface.layer() {
                            Layer::Background => niri_ipc::Layer::Background,
                            Layer::Bottom => niri_ipc::Layer::Bottom,
                            Layer::Top => niri_ipc::Layer::Top,
                            Layer::Overlay => niri_ipc::Layer::Overlay,
                        };
                        let keyboard_interactivity =
                            match surface.cached_state().keyboard_interactivity {
                                KeyboardInteractivity::None => {
                                    niri_ipc::LayerSurfaceKeyboardInteractivity::None
                                }
                                KeyboardInteractivity::Exclusive => {
                                    niri_ipc::LayerSurfaceKeyboardInteractivity::Exclusive
                                }
                                KeyboardInteractivity::OnDemand => {
                                    niri_ipc::LayerSurfaceKeyboardInteractivity::OnDemand
                                }
                            };

                        layers.push(niri_ipc::LayerSurface {
                            namespace: surface.namespace().to_owned(),
                            output: name.clone(),
                            layer,
                            keyboard_interactivity,
                        });
                    }
                }

                let _ = tx.send_blocking(layers);
            });
            let result = rx.recv().await;
            let layers = result.map_err(|_| String::from("error getting layers info"))?;
            Response::Layers(layers)
        }
        Request::KeyboardLayouts => {
            let state = ctx.event_stream_state.borrow();
            let layout = state.keyboard_layouts.keyboard_layouts.clone();
            let layout = layout.expect("keyboard layouts should be set at startup");
            Response::KeyboardLayouts(layout)
        }
        Request::FocusedWindow => {
            let state = ctx.event_stream_state.borrow();
            let windows = &state.windows.windows;
            let window = windows.values().find(|win| win.is_focused).cloned();
            Response::FocusedWindow(window)
        }
        Request::WindowUnderCursor => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let id = state
                    .niri
                    .window_under_cursor()
                    .map(|window| window.id().get());
                let _ = tx.send_blocking(id);
            });
            let id = rx
                .recv()
                .await
                .map_err(|_| String::from("error reading window under pointer"))?;
            let window = id.and_then(|id| {
                ctx.event_stream_state
                    .borrow()
                    .windows
                    .windows
                    .get(&id)
                    .cloned()
            });
            Response::WindowUnderCursor(window)
        }
        Request::PickWindow => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let pointer = state.niri.seat.get_pointer().unwrap();
                let start_data = PointerGrabStartData {
                    focus: None,
                    button: 0,
                    location: pointer.current_location(),
                };
                let grab = PickWindowGrab::new(start_data);
                // The `WindowPickGrab` ungrab handler will cancel the previous ongoing pick, if
                // any.
                pointer.set_grab(state, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
                state.niri.pick_window = Some(tx);
                state
                    .niri
                    .cursor_manager
                    .set_cursor_image(CursorImageStatus::Named(CursorIcon::Crosshair));
                // Redraw to update the cursor.
                state.niri.queue_redraw_all();
            });
            let result = rx.recv().await;
            let id = result.map_err(|_| String::from("error getting picked window info"))?;
            let window = id.and_then(|id| {
                let state = ctx.event_stream_state.borrow();
                state.windows.windows.get(&id.get()).cloned()
            });
            Response::PickedWindow(window)
        }
        Request::PickColor => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                state.handle_pick_color(tx);
            });
            let result = rx.recv().await;
            let color = result.map_err(|_| String::from("error getting picked color"))?;
            Response::PickedColor(color)
        }
        Request::Action(action) => {
            validate_action(&action)?;

            let (tx, rx) = async_channel::bounded(1);

            let action = niri_config::Action::from(action);
            ctx.event_loop.insert_idle(move |state| {
                // Make sure some logic like workspace clean-up has a chance to run before doing
                // actions.
                state.niri.advance_animations();
                state.do_action(action, false);
                let _ = tx.send_blocking(());
            });

            // Wait until the action has been processed before returning. This is important for a
            // few actions, for instance for DoScreenTransition this wait ensures that the screen
            // contents were sampled into the texture.
            let _ = rx.recv().await;
            Response::Handled
        }
        Request::Output { output, action } => {
            action.validate()?;

            let ipc_outputs = ctx.ipc_outputs.lock().unwrap();
            let found = ipc_outputs
                .values()
                .any(|o| OutputName::from_ipc_output(o).matches(&output));
            let response = if found {
                OutputConfigChanged::Applied
            } else {
                OutputConfigChanged::OutputWasMissing
            };
            drop(ipc_outputs);

            ctx.event_loop.insert_idle(move |state| {
                state.apply_transient_output_config(&output, action);
            });

            Response::OutputConfigChanged(response)
        }
        Request::FocusedOutput => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let active_output = state
                    .niri
                    .layout
                    .active_output()
                    .map(|output| output.name());

                let output = active_output.and_then(|active_output| {
                    state
                        .backend
                        .ipc_outputs()
                        .lock()
                        .unwrap()
                        .values()
                        .find(|o| o.name == active_output)
                        .cloned()
                });

                let _ = tx.send_blocking(output);
            });
            let result = rx.recv().await;
            let output = result.map_err(|_| String::from("error getting active output info"))?;
            Response::FocusedOutput(output)
        }
        Request::EventStream | Request::EventStreamWithCollections => Response::Handled,
        Request::NacreOverviewHoverStream => {
            let (tx, rx) = async_channel::bounded(1);
            ctx.event_loop.insert_idle(move |state| {
                let enabled = state.niri.config.borrow().nacre.overview_hover;
                let id = if enabled
                    && state.niri.layout.is_overview_open()
                    && state.niri.pointer_visibility.is_visible()
                {
                    state
                        .niri
                        .window_under_cursor()
                        .map(|window| window.id().get())
                } else {
                    None
                };
                if let Some(server) = state.niri.ipc_server.as_ref() {
                    server.update_overview_hover(id);
                }
                let _ = tx.send_blocking(enabled);
            });
            if !rx.recv().await.unwrap_or(false) {
                return Err(String::from("Nacre overview hover is disabled"));
            }
            Response::Handled
        }
        Request::OverviewState => {
            let state = ctx.event_stream_state.borrow();
            let is_open = state.overview.is_open;
            Response::OverviewState(Overview { is_open })
        }
        Request::Casts => {
            let state = ctx.event_stream_state.borrow();
            let casts = state.casts.casts.values().cloned().collect();
            Response::Casts(casts)
        }
    };

    Ok(response)
}

fn validate_action(action: &Action) -> Result<(), String> {
    if let Action::Screenshot { path, .. }
    | Action::ScreenshotScreen { path, .. }
    | Action::ScreenshotWindow { path, .. }
    | Action::LoadConfigFile { path } = action
    {
        if let Some(path) = path {
            // Relative paths are resolved against the niri compositor's working directory, which
            // is almost certainly not what you want.
            if !Path::new(path).is_absolute() {
                return Err(format!("path must be absolute: {path}"));
            }
        }
    }

    if let Action::LoadConfigFile { path: Some(path) } = action {
        let p = Path::new(path);
        if !p.is_file() {
            return Err(format!("path does not point to a file: {path}"));
        }
    }

    Ok(())
}

async fn handle_event_stream_client(client: EventStreamClient) -> anyhow::Result<()> {
    let EventStreamClient {
        events,
        disconnect,
        mut write,
    } = client;

    while let Ok(event) = events.recv().await {
        let mut buf = serde_json::to_vec(&event).context("error formatting event")?;
        buf.push(b'\n');

        let res = select_biased! {
            _ = disconnect.recv().fuse() => return Ok(()),
            res = write.write_all(&buf).fuse() => res,
        };

        match res {
            Ok(()) => (),
            // Normal client disconnection.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            res @ Err(_) => res.context("error writing event")?,
        }
    }

    Ok(())
}

fn make_ipc_window(
    mapped: &Mapped,
    workspace_id: Option<WorkspaceId>,
    layout: WindowLayout,
) -> niri_ipc::Window {
    with_toplevel_role(mapped.toplevel(), |role| niri_ipc::Window {
        id: mapped.id().get(),
        title: role.title.clone(),
        app_id: role.app_id.clone(),
        pid: mapped.credentials().map(|c| c.pid),
        workspace_id: workspace_id.map(|id| id.get()),
        is_focused: mapped.is_focused(),
        is_floating: mapped.is_floating(),
        is_urgent: mapped.is_urgent(),
        layout,
        focus_timestamp: mapped.get_focus_timestamp().map(Timestamp::from),
    })
}

impl State {
    pub fn ipc_keyboard_layouts_changed(&mut self) {
        let keyboard = self.niri.seat.get_keyboard().unwrap();
        let keyboard_layouts = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap();
            let layouts = xkb.layouts();
            KeyboardLayouts {
                names: layouts
                    .map(|layout| xkb.layout_name(layout).to_owned())
                    .collect(),
                current_idx: xkb.active_layout().0 as u8,
            }
        });

        let Some(server) = &self.niri.ipc_server else {
            return;
        };

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.keyboard_layouts;

        let event = Event::KeyboardLayoutsChanged { keyboard_layouts };
        state.apply(event.clone());
        server.send_event(event);
    }

    pub fn ipc_refresh_keyboard_layout_index(&mut self) {
        let keyboard = self.niri.seat.get_keyboard().unwrap();
        let idx = keyboard.with_xkb_state(self, |context| {
            let xkb = context.xkb().lock().unwrap();
            xkb.active_layout().0 as u8
        });

        let Some(server) = &self.niri.ipc_server else {
            return;
        };

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.keyboard_layouts;

        if state.keyboard_layouts.as_ref().unwrap().current_idx == idx {
            return;
        }

        let event = Event::KeyboardLayoutSwitched { idx };
        state.apply(event.clone());
        server.send_event(event);
    }

    pub fn ipc_refresh_layout(&mut self) {
        self.ipc_refresh_workspaces();
        self.ipc_refresh_collections();
        self.ipc_refresh_windows();
        self.ipc_refresh_overview();
    }

    fn ipc_refresh_collections(&mut self) {
        let Some(server) = &self.niri.ipc_server else {
            return;
        };
        let collections = self.niri.layout.collections();
        let mut state = server.event_stream_state.borrow_mut();
        if state.collections.collections != collections {
            let event = Event::CollectionsChanged { collections };
            state.collections.apply(event.clone());
            server.send_event(event);
        }
    }

    fn ipc_refresh_workspaces(&mut self) {
        let Some(server) = &self.niri.ipc_server else {
            return;
        };

        let _span = tracy_client::span!("State::ipc_refresh_workspaces");

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.workspaces;

        let mut events = Vec::new();
        let layout = &self.niri.layout;
        let focused_ws_id = layout.active_workspace().map(|ws| ws.id().get());

        // Check for workspace changes.
        let mut seen = HashSet::new();
        let mut need_workspaces_changed = false;
        for (mon, ws_idx, ws) in layout.workspaces() {
            let id = ws.id().get();
            seen.insert(id);

            let Some(ipc_ws) = state.workspaces.get(&id) else {
                // A new workspace was added.
                need_workspaces_changed = true;
                break;
            };

            // Check for any changes that we can't signal as individual events.
            let output_name = mon.map(|mon| mon.output_name());
            if ipc_ws.idx != u8::try_from(ws_idx + 1).unwrap_or(u8::MAX)
                || ipc_ws.name.as_ref() != ws.name()
                || ipc_ws.output.as_ref() != output_name
            {
                need_workspaces_changed = true;
                break;
            }

            let active_window_id = ws.active_window().map(|win| win.id().get());
            if ipc_ws.active_window_id != active_window_id {
                events.push(Event::WorkspaceActiveWindowChanged {
                    workspace_id: id,
                    active_window_id,
                });
            }

            // Check if this workspace urgent state changed.
            let urgent = ws.is_urgent();
            if urgent != ipc_ws.is_urgent {
                events.push(Event::WorkspaceUrgencyChanged { id, urgent });
            }

            // Check if this workspace became focused.
            let is_focused = Some(id) == focused_ws_id;
            if is_focused && !ipc_ws.is_focused {
                events.push(Event::WorkspaceActivated { id, focused: true });
                continue;
            }

            // Check if this workspace became active.
            let is_active = mon.is_some_and(|mon| mon.active_workspace_idx() == ws_idx);
            if is_active && !ipc_ws.is_active {
                events.push(Event::WorkspaceActivated { id, focused: false });
            }
        }

        // Check if any workspaces were removed.
        if !need_workspaces_changed && state.workspaces.keys().any(|id| !seen.contains(id)) {
            need_workspaces_changed = true;
        }

        if need_workspaces_changed {
            events.clear();

            let workspaces = layout
                .workspaces()
                .map(|(mon, ws_idx, ws)| {
                    let id = ws.id().get();
                    Workspace {
                        id,
                        idx: u8::try_from(ws_idx + 1).unwrap_or(u8::MAX),
                        name: ws.name().cloned(),
                        output: mon.map(|mon| mon.output_name().clone()),
                        is_urgent: ws.is_urgent(),
                        is_active: mon.is_some_and(|mon| mon.active_workspace_idx() == ws_idx),
                        is_focused: Some(id) == focused_ws_id,
                        active_window_id: ws.active_window().map(|win| win.id().get()),
                    }
                })
                .collect();

            events.push(Event::WorkspacesChanged { workspaces });
        }

        for event in events {
            state.apply(event.clone());
            server.send_event(event);
        }
    }

    fn ipc_refresh_windows(&mut self) {
        let Some(server) = &self.niri.ipc_server else {
            return;
        };

        let _span = tracy_client::span!("State::ipc_refresh_windows");

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.windows;

        let mut events = Vec::new();
        let layout = &self.niri.layout;

        let mut batch_change_layouts: Vec<(u64, WindowLayout)> = Vec::new();

        // Check for window changes.
        let mut seen = HashSet::new();
        let mut focused_id = None;
        layout.with_windows(|mapped, _, ws_id, window_layout| {
            let id = mapped.id().get();
            seen.insert(id);

            if mapped.is_focused() {
                focused_id = Some(id);
            }

            let Some(ipc_win) = state.windows.get(&id) else {
                let window = make_ipc_window(mapped, ws_id, window_layout);
                events.push(Event::WindowOpenedOrChanged { window });
                return;
            };

            let workspace_id = ws_id.map(|id| id.get());
            let mut changed =
                ipc_win.workspace_id != workspace_id || ipc_win.is_floating != mapped.is_floating();

            changed |= with_toplevel_role(mapped.toplevel(), |role| {
                ipc_win.title != role.title || ipc_win.app_id != role.app_id
            });

            if changed {
                let window = make_ipc_window(mapped, ws_id, window_layout);
                events.push(Event::WindowOpenedOrChanged { window });
                return;
            }

            if ipc_win.layout != window_layout {
                batch_change_layouts.push((id, window_layout));
            }

            if mapped.is_focused() && !ipc_win.is_focused {
                events.push(Event::WindowFocusChanged { id: Some(id) });
            }

            let focus_timestamp = mapped.get_focus_timestamp().map(Timestamp::from);
            if focus_timestamp != ipc_win.focus_timestamp {
                events.push(Event::WindowFocusTimestampChanged {
                    id,
                    focus_timestamp,
                });
            }

            let urgent = mapped.is_urgent();
            if urgent != ipc_win.is_urgent {
                events.push(Event::WindowUrgencyChanged { id, urgent })
            }
        });

        // It might make sense to push layout changes after closed windows (since windows about to
        // be closed will occupy the same column/tile positions as the window that moved into this
        // vacated space), but also we are already pushing some layout changes in
        // WindowOpenedOrChanged above, meaning that the receiving end has to handle this case
        // anyway.
        if !batch_change_layouts.is_empty() {
            events.push(Event::WindowLayoutsChanged {
                changes: batch_change_layouts,
            });
        }

        // Check for closed windows.
        let mut ipc_focused_id = None;
        for (id, ipc_win) in &state.windows {
            if !seen.contains(id) {
                events.push(Event::WindowClosed { id: *id });
            }

            if ipc_win.is_focused {
                ipc_focused_id = Some(id);
            }
        }

        // Extra check for focus becoming None, since the checks above only work for focus becoming
        // a different window.
        if focused_id.is_none() && ipc_focused_id.is_some() {
            events.push(Event::WindowFocusChanged { id: None });
        }

        for event in events {
            state.apply(event.clone());
            server.send_event(event);
        }
    }

    pub fn ipc_refresh_overview(&mut self) {
        let Some(server) = &self.niri.ipc_server else {
            return;
        };

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.overview;
        let is_open = self.niri.layout.is_overview_open();

        if state.is_open == is_open {
            return;
        }

        let event = Event::OverviewOpenedOrClosed { is_open };
        state.apply(event.clone());
        server.send_event(event);
    }

    pub fn ipc_refresh_casts(&mut self) {
        let Some(server) = &self.niri.ipc_server else {
            return;
        };

        let _span = tracy_client::span!("State::ipc_refresh_casts");

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.casts;

        let mut events = Vec::new();
        let mut seen = HashSet::new();

        // Check PipeWire screencasts.
        #[cfg(feature = "xdp-gnome-screencast")]
        {
            // Check pending dynamic casts.
            for pending in &self.niri.casting.pending_dynamic_casts {
                let stream_id = pending.stream_id.get();
                seen.insert(stream_id);

                // Pending dynamic casts don't change any properties, so we only need to check if
                // it's missing from the state.
                if !state.casts.contains_key(&stream_id) {
                    let cast = niri_ipc::Cast {
                        session_id: pending.session_id.get(),
                        stream_id,
                        kind: niri_ipc::CastKind::PipeWire,
                        target: niri_ipc::CastTarget::Nothing {},
                        is_dynamic_target: true,
                        is_active: false,
                        pid: None,
                        pw_node_id: None,
                    };
                    events.push(Event::CastStartedOrChanged { cast });
                }
            }

            // Check active casts.
            for cast in &self.niri.casting.casts {
                let stream_id = cast.stream_id.get();
                seen.insert(stream_id);

                let pw_node_id = cast.node_id();
                if state.casts.get(&stream_id).is_none_or(|existing| {
                    // Only these properties can change.
                    existing.is_active != cast.is_active()
                        || !cast.target.matches(&existing.target)
                        || existing.pw_node_id != pw_node_id
                }) {
                    let cast = niri_ipc::Cast {
                        session_id: cast.session_id.get(),
                        stream_id,
                        kind: niri_ipc::CastKind::PipeWire,
                        target: cast.target.make_ipc(),
                        is_dynamic_target: cast.dynamic_target,
                        is_active: cast.is_active(),
                        pid: None,
                        pw_node_id,
                    };
                    events.push(Event::CastStartedOrChanged { cast });
                }
            }
        }

        // Check screencopy casts.
        //
        // First, clear expired casts. Ideally we'd have a deadline timer, but our 1 second frame
        // callback timer calls refresh regularly, so that's fine as is.
        self.niri.screencopy_state.clear_expired_casts();

        for queue in self.niri.screencopy_state.queues() {
            if let Some(cast_info) = queue.cast() {
                let stream_id = cast_info.stream_id.get();
                seen.insert(stream_id);

                if state.casts.get(&stream_id).is_none_or(|existing| {
                    // Only this property can change.
                    match &existing.target {
                        niri_ipc::CastTarget::Output { name } => *name != cast_info.output_name,
                        _ => true,
                    }
                }) {
                    let cast = niri_ipc::Cast {
                        session_id: cast_info.session_id.get(),
                        stream_id,
                        kind: niri_ipc::CastKind::WlrScreencopy,
                        target: niri_ipc::CastTarget::Output {
                            name: cast_info.output_name.clone(),
                        },
                        is_dynamic_target: false,
                        is_active: true,
                        pid: queue.credentials().map(|creds| creds.pid),
                        pw_node_id: None,
                    };
                    events.push(Event::CastStartedOrChanged { cast });
                }
            }
        }

        // Check for stopped casts.
        for stream_id in state.casts.keys() {
            if !seen.contains(stream_id) {
                events.push(Event::CastStopped {
                    stream_id: *stream_id,
                });
            }
        }

        for event in events {
            state.apply(event.clone());
            server.send_event(event);
        }
    }

    pub fn ipc_config_loaded(&mut self, failed: bool) {
        let Some(server) = &self.niri.ipc_server else {
            return;
        };
        let mut state = server.event_stream_state.borrow_mut();

        let event = Event::ConfigLoaded { failed };
        state.apply(event.clone());
        server.send_event(event);
    }

    pub fn ipc_screenshot_taken(&mut self, path: Option<String>) {
        let Some(server) = &self.niri.ipc_server else {
            return;
        };
        let mut state = server.event_stream_state.borrow_mut();

        let event = Event::ScreenshotCaptured { path };
        state.apply(event.clone());
        server.send_event(event);
    }
}

#[cfg(test)]
mod nacre_hover_tests {
    use super::*;

    #[test]
    fn overview_hover_is_opt_in_and_does_not_leak_to_regular_streams() {
        let server = IpcServer {
            socket_path: None,
            event_streams: Rc::new(RefCell::new(Vec::new())),
            event_stream_state: Rc::new(RefCell::new(EventStreamState::default())),
            overview_hover: Rc::new(Cell::new(None)),
        };
        let (regular_tx, regular_rx) = async_channel::bounded(4);
        let (hover_tx, hover_rx) = async_channel::bounded(4);
        let (regular_disconnect, _) = async_channel::bounded(1);
        let (hover_disconnect, _) = async_channel::bounded(1);
        server.event_streams.borrow_mut().extend([
            EventStreamSender {
                overview_hover: false,
                events: regular_tx,
                disconnect: regular_disconnect,
                include_collections: false,
            },
            EventStreamSender {
                overview_hover: true,
                events: hover_tx,
                disconnect: hover_disconnect,
                include_collections: false,
            },
        ]);

        assert!(server.has_overview_hover_clients());
        server.update_overview_hover(Some(7));
        assert!(regular_rx.try_recv().is_err());
        assert!(matches!(
            hover_rx.try_recv(),
            Ok(Event::NacreOverviewHoverChanged { id: Some(7) })
        ));
        server.update_overview_hover(Some(7));
        assert!(hover_rx.try_recv().is_err());
        server.send_event(Event::OverviewOpenedOrClosed { is_open: true });
        for receiver in [&regular_rx, &hover_rx] {
            assert!(matches!(
                receiver.try_recv(),
                Ok(Event::OverviewOpenedOrClosed { is_open: true })
            ));
        }
        server.update_overview_hover(None);
        assert!(regular_rx.try_recv().is_err());
        assert!(matches!(
            hover_rx.try_recv(),
            Ok(Event::NacreOverviewHoverChanged { id: None })
        ));
    }
}
