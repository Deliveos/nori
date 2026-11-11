use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::{env, io, process};

use anyhow::Context;
use async_channel::{Receiver, Sender};
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
use smithay::backend::input::InputTime;
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
use crate::handlers::image_copy_capture::{source_mapped_id, source_output};
use crate::input::pick_window_grab::PickWindowGrab;
use crate::layout::workspace::WorkspaceId;
use crate::niri::State;
use crate::utils::listening_socket::{insert_listening_socket, OnAcceptError};
use crate::utils::{version, with_toplevel_role};
use crate::window::Mapped;

// If an event stream client fails to read events fast enough that we accumulate more than this
// many bytes of events in our buffer, we drop that event stream client.
//
// The client tasks run on our event loop, so they can't write anything while one refresh produces
// its events. The limit is in bytes rather than events so that such a burst fits, like one event
// for each window when many windows change at once.
const EVENT_STREAM_BUFFER_SIZE: usize = 1024 * 1024;

// A client that sends more than this without a newline is dropped, so that it can't make us buffer
// without limit. Spawn requests carry a whole argv, which Linux allows up to 6 MiB with a large
// stack limit, so leave room for that plus JSON escaping.
const MAX_REQUEST_SIZE: u64 = 8 * 1024 * 1024;

pub struct IpcServer {
    /// Path to the IPC socket.
    ///
    /// This is `None` when creating `IpcServer` without a socket.
    pub socket_path: Option<PathBuf>,
    event_streams: Rc<RefCell<Vec<EventStreamSender>>>,
    event_stream_state: Rc<RefCell<EventStreamState>>,
}

struct ClientCtx {
    event_loop: LoopHandle<'static, State>,
    scheduler: Scheduler<()>,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
    event_streams: Rc<RefCell<Vec<EventStreamSender>>>,
    event_stream_state: Rc<RefCell<EventStreamState>>,
}

struct EventStreamClient {
    /// Serialized events, each ending with a newline.
    events: Receiver<Rc<[u8]>>,
    /// Bytes of events in `events` and being written.
    queued_bytes: Rc<Cell<usize>>,
    disconnect: Receiver<()>,
    write: Box<dyn AsyncWrite + Unpin>,
}

struct EventStreamSender {
    events: Sender<Rc<[u8]>>,
    queued_bytes: Rc<Cell<usize>>,
    disconnect: Sender<()>,
}

impl EventStreamSender {
    /// Queues a serialized event, returns `false` if the client should be dropped.
    fn send(&self, event: Rc<[u8]>) -> bool {
        if self.queued_bytes.get() > EVENT_STREAM_BUFFER_SIZE {
            warn!("disconnecting IPC event stream client because it is reading events too slowly");
            return false;
        }

        self.queue(event)
    }

    /// Queues a serialized event without checking the limit, returns `false` if the client is
    /// gone.
    fn queue(&self, event: Rc<[u8]>) -> bool {
        let len = event.len();
        // The channel is unbounded, so this fails only when it's closed.
        if self.events.try_send(event).is_err() {
            return false;
        }

        self.queued_bytes.set(self.queued_bytes.get() + len);
        true
    }
}

fn serialize_event(event: &Event) -> anyhow::Result<Rc<[u8]>> {
    let mut buf = serde_json::to_vec(event).context("error formatting event")?;
    buf.push(b'\n');
    Ok(Rc::from(buf))
}

impl IpcServer {
    pub fn start(
        event_loop: &LoopHandle<'static, State>,
        wayland_socket_name: Option<&OsStr>,
    ) -> anyhow::Result<Self> {
        let _span = tracy_client::span!("Ipc::start");

        let socket_path = if let Some(wayland_socket_name) = wayland_socket_name {
            let wayland_socket_name = wayland_socket_name.to_string_lossy();
            let socket_name = format!("nori.{wayland_socket_name}.{}.sock", process::id());
            let mut socket_path = socket_dir();
            socket_path.push(socket_name);

            let listener = UnixListener::bind(&socket_path).context("error binding socket")?;
            listener
                .set_nonblocking(true)
                .context("error setting socket to non-blocking")?;

            let source = Generic::new(listener, Interest::READ, Mode::Level);
            insert_listening_socket(
                event_loop,
                String::from("the IPC socket"),
                OnAcceptError::Pause,
                source,
                |_, socket, state| {
                    match socket.accept() {
                        Ok((stream, _)) => on_new_ipc_client(state, stream),
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                        Err(e) => return Err(e),
                    }

                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| err.error)
            .context("error adding the IPC socket to the event loop")?;

            Some(socket_path)
        } else {
            None
        };

        Ok(Self {
            socket_path,
            event_streams: Rc::new(RefCell::new(Vec::new())),
            event_stream_state: Rc::new(RefCell::new(EventStreamState::default())),
        })
    }

    #[cfg(test)]
    pub fn event_stream_state(&self) -> std::cell::Ref<'_, EventStreamState> {
        self.event_stream_state.borrow()
    }

    #[cfg(test)]
    pub fn event_stream_count(&self) -> usize {
        self.event_streams.borrow().len()
    }

    fn send_event(&self, event: Event) {
        let mut streams = self.event_streams.borrow_mut();
        if streams.is_empty() {
            return;
        }

        // Serialize once for all clients.
        let event = match serialize_event(&event) {
            Ok(event) => event,
            Err(err) => {
                warn!("error sending IPC event: {err:?}");
                return;
            }
        };

        let mut to_remove = Vec::new();
        for (idx, stream) in streams.iter().enumerate() {
            if !stream.send(event.clone()) {
                to_remove.push(idx);
            }
        }

        for idx in to_remove.into_iter().rev() {
            let stream = streams.swap_remove(idx);
            let _ = stream.disconnect.send_blocking(());
        }
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

pub(crate) fn on_new_ipc_client(state: &mut State, stream: UnixStream) {
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
        let res = (&mut read)
            .take(MAX_REQUEST_SIZE)
            .read_until(b'\n', &mut buf)
            .await;
        match res {
            Ok(0) => return Ok(()),
            Ok(_) => (),
            // Normal client disconnection.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(err) => {
                return Err(err).context("error reading request");
            }
        }
        // The rest of the request is still unread, so we can't go on with this client. Tell it why
        // first: a client that reads gets the reply before the connection closes.
        if buf.len() as u64 == MAX_REQUEST_SIZE && buf.last() != Some(&b'\n') {
            drop(buf);
            let err = format!("request is longer than {MAX_REQUEST_SIZE} bytes");
            let mut reply = serde_json::to_vec(&Reply::Err(err.clone())).unwrap_or_default();
            reply.push(b'\n');
            let _ = write.write_all(&reply).await;
            anyhow::bail!(err);
        }

        let request = serde_json::from_slice(&buf)
            .context("error parsing request")
            .map_err(|err| err.to_string());
        let requested_error = matches!(request, Ok(Request::ReturnError));
        let requested_event_stream = matches!(request, Ok(Request::EventStream));

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

        if requested_event_stream {
            let (events_tx, events_rx) = async_channel::unbounded();
            let (disconnect_tx, disconnect_rx) = async_channel::bounded(1);
            let queued_bytes = Rc::new(Cell::new(0));

            // Spawn a task for the client.
            let client = EventStreamClient {
                events: events_rx,
                queued_bytes: queued_bytes.clone(),
                disconnect: disconnect_rx,
                write: Box::new(write) as _,
            };
            let sender = EventStreamSender {
                events: events_tx,
                queued_bytes,
                disconnect: disconnect_tx,
            };
            let future = async move {
                if let Err(err) = handle_event_stream_client(client).await {
                    warn!("error handling IPC event stream client: {err:?}");
                }
            };
            if let Err(err) = ctx.scheduler.schedule(future) {
                warn!("error scheduling IPC event stream future: {err:?}");
            }

            // Send the initial state. It can be larger than the limit with many windows, so
            // don't check the limit here.
            {
                let state = ctx.event_stream_state.borrow();
                for event in state.replicate() {
                    let event = serialize_event(&event)?;
                    if !sender.queue(event) {
                        return Ok(());
                    }
                }
            }

            // Add it to the list.
            ctx.event_streams.borrow_mut().push(sender);

            return Ok(());
        }
    }
}

async fn process(ctx: &ClientCtx, request: Request) -> Reply {
    let response = match request {
        Request::ReturnError => return Err(String::from("example compositor error")),
        Request::Version => Response::Version(version()),
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
            validate_action(&action).await?;

            let (tx, rx) = async_channel::bounded(1);

            let action = niri_config::Action::from(action);
            ctx.event_loop.insert_idle(move |state| {
                // Make sure some logic like workspace clean-up has a chance to run before doing
                // actions.
                state.niri.advance_animations();
                // This action didn't come from an input, so just use the current time.
                let time = InputTime::now();
                state.do_action(action, false, time);
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
        Request::EventStream => Response::Handled,
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

async fn validate_action(action: &Action) -> Result<(), String> {
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

    if let Action::SetWorkspaceName { name, .. } = action {
        if name.contains('\0') {
            return Err(String::from("workspace name must not contain a NUL byte"));
        }
    }

    if let Action::LoadConfigFile { path: Some(path) } = action {
        if !is_file_on_thread(path).await? {
            return Err(format!("path does not point to a file: {path}"));
        }
    }

    Ok(())
}

/// Checks whether the path is a file on a thread, since `stat` can block for a long time, for
/// example on a stalled network mount.
async fn is_file_on_thread(path: &str) -> Result<bool, String> {
    let (tx, rx) = async_channel::bounded(1);
    let path = PathBuf::from(path);
    std::thread::Builder::new()
        .name("IPC Path Check".to_owned())
        .spawn(move || {
            let _ = tx.send_blocking(path.is_file());
        })
        .map_err(|err| format!("error spawning a thread to check the path: {err}"))?;

    rx.recv()
        .await
        .map_err(|_| String::from("error checking the path"))
}

async fn handle_event_stream_client(client: EventStreamClient) -> anyhow::Result<()> {
    let EventStreamClient {
        events,
        queued_bytes,
        disconnect,
        mut write,
    } = client;

    while let Ok(event) = events.recv().await {
        let res = select_biased! {
            _ = disconnect.recv().fuse() => return Ok(()),
            res = write.write_all(&event).fuse() => res,
        };
        queued_bytes.set(queued_bytes.get().saturating_sub(event.len()));

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
        self.ipc_refresh_windows();
        self.ipc_refresh_overview();
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
        //
        // Count the windows that the state already has, so that closed windows can be detected
        // without collecting every id on every iteration.
        let mut seen_known = 0;
        let mut focused_id = None;
        layout.with_windows(|mapped, _, ws_id, window_layout| {
            let id = mapped.id().get();

            if mapped.is_focused() {
                focused_id = Some(id);
            }

            let Some(ipc_win) = state.windows.get(&id) else {
                let window = make_ipc_window(mapped, ws_id, window_layout);
                events.push(Event::WindowOpenedOrChanged { window });
                return;
            };
            seen_known += 1;

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

        // Check for closed windows. If every window in the state was seen, none were closed.
        let alive = (seen_known < state.windows.len())
            .then(|| HashSet::<u64>::from_iter(layout.windows().map(|(_, win)| win.id().get())));
        let mut ipc_focused_id = None;
        for (id, ipc_win) in &state.windows {
            if alive.as_ref().is_some_and(|alive| !alive.contains(id)) {
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
        let _span = tracy_client::span!("State::ipc_refresh_casts");

        // Clear expired screencopy casts. Ideally we'd have a deadline timer, but our 1 second
        // frame callback timer calls refresh regularly, so that's fine as is. This also frees idle
        // screencopy textures, so it runs even without the IPC server.
        self.niri.screencopy_state.clear_expired_casts();

        let Some(server) = &self.niri.ipc_server else {
            return;
        };

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

        // Check screencopy casts (expired ones were cleared above).
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

        // Check ext-image-copy-capture casts.
        //
        // Unlike wlr-screencopy, a session is an explicit protocol object, so
        // the cast lifetime is tied to it. Dead sessions are already dropped by
        // refresh_image_copy_capture(), which runs before this. Sessions which
        // have requested only one frame are screenshots, not casts.
        let output_sessions = self
            .niri
            .image_copy_sessions
            .iter()
            .filter(|s| s.is_cast())
            .map(|s| {
                (
                    niri_ipc::CastKind::ExtImageCopyCapture,
                    s.session_id,
                    s.stream_id,
                    s.session.source(),
                    s.credentials,
                )
            });
        let cursor_sessions = self
            .niri
            .image_copy_cursor_sessions
            .iter()
            .filter(|s| s.is_cast())
            .map(|s| {
                (
                    niri_ipc::CastKind::ExtImageCopyCaptureCursor,
                    s.session_id,
                    s.stream_id,
                    s.session.source(),
                    s.credentials,
                )
            });
        for (kind, session_id, stream_id, source, credentials) in
            output_sessions.chain(cursor_sessions)
        {
            let target = if let Some(output) = source_output(&source) {
                niri_ipc::CastTarget::Output {
                    name: output.name(),
                }
            } else if let Some(id) = source_mapped_id(&source) {
                niri_ipc::CastTarget::Window { id: id.get() }
            } else {
                niri_ipc::CastTarget::Nothing {}
            };

            let stream_id = stream_id.get();
            seen.insert(stream_id);

            if !state.casts.contains_key(&stream_id) {
                let cast = niri_ipc::Cast {
                    session_id: session_id.get(),
                    stream_id,
                    kind,
                    target,
                    is_dynamic_target: false,
                    is_active: true,
                    pid: credentials.map(|creds| creds.pid),
                    pw_node_id: None,
                };
                events.push(Event::CastStartedOrChanged { cast });
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

    pub fn ipc_refresh_touchpad(&mut self) {
        let Some(server) = &self.niri.ipc_server else {
            return;
        };

        let mut state = server.event_stream_state.borrow_mut();
        let state = &mut state.touchpad;
        let on = !self.niri.touchpad_disabled_by_toggle;

        if state.on == on {
            return;
        }

        let event = Event::TouchpadToggled { on };
        state.apply(event.clone());
        server.send_event(event);
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
mod tests {
    use super::*;

    #[test]
    fn workspace_name_with_nul_is_refused() {
        let action = |name: &str| Action::SetWorkspaceName {
            name: String::from(name),
            workspace: None,
        };
        // Checking this action doesn't wait for anything, so the future is ready right away.
        let validate = |action| validate_action(&action).now_or_never().unwrap();
        assert!(validate(action("a\0b")).is_err());
        assert_eq!(validate(action("ab")), Ok(()));
    }
}
