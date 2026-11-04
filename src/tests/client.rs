use std::cmp::min;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write as _;
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1};
use smithay::reexports::wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1};
use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1};
use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::{self, ExtSessionLockV1};
use smithay::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1;
use smithay::reexports::wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibitor_v1::{self, ZwpKeyboardShortcutsInhibitorV1};
use smithay::reexports::wayland_protocols::wp::security_context::v1::client::wp_security_context_manager_v1::WpSecurityContextManagerV1;
use smithay::reexports::wayland_protocols::wp::security_context::v1::client::wp_security_context_v1::WpSecurityContextV1;
use smithay::reexports::wayland_protocols::wp::single_pixel_buffer;
use smithay::reexports::wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use smithay::reexports::wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3;
use smithay::reexports::wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay::reexports::wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_popup::{self, XdgPopup};
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_positioner::{self, XdgPositioner};
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    self, ZwlrLayerShellV1,
};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::client::zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1;
use smithay::reexports::wayland_protocols_wlr::output_management::v1::client::zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::client::zwlr_output_head_v1::{self, ZwlrOutputHeadV1};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::client::zwlr_output_manager_v1::{self, ZwlrOutputManagerV1};
use smithay::reexports::wayland_protocols_wlr::output_management::v1::client::zwlr_output_mode_v1::{self, ZwlrOutputModeV1};
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use smithay::reexports::wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;
use smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
use smithay::reexports::wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2;
use smithay::reexports::rustix::fs::{memfd_create, MemfdFlags};
use wayland_backend::client::Backend;
use wayland_client::globals::Global;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_display::WlDisplay;
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, Proxy as _, QueueHandle};

use crate::utils::id::IdCounter;

pub struct Client {
    pub id: ClientId,
    pub event_loop: EventLoop<'static, State>,
    pub connection: Connection,
    pub qh: QueueHandle<State>,
    pub display: WlDisplay,
    pub registry: WlRegistry,
    pub state: State,
    /// Whether the test expects niri to post a protocol error to this client.
    pub expect_protocol_error: bool,
}

pub struct State {
    pub qh: QueueHandle<State>,

    pub globals: Vec<Global>,
    pub outputs: HashMap<WlOutput, String>,
    pub surfaces: HashMap<WlSurface, Surface>,
    pub seats: HashMap<WlSeat, Seat>,

    pub compositor: Option<WlCompositor>,
    pub subcompositor: Option<WlSubcompositor>,
    pub xdg_wm_base: Option<XdgWmBase>,
    pub layer_shell: Option<ZwlrLayerShellV1>,
    pub session_lock_manager: Option<ExtSessionLockManagerV1>,
    pub virtual_pointer_manager: Option<ZwlrVirtualPointerManagerV1>,
    pub virtual_pointer: Option<ZwlrVirtualPointerV1>,
    pub spbm: Option<WpSinglePixelBufferManagerV1>,
    pub shm: Option<WlShm>,
    pub text_input_manager: Option<ZwpTextInputManagerV3>,
    pub input_method_manager: Option<ZwpInputMethodManagerV2>,
    pub viewporter: Option<WpViewporter>,
    pub ksim: Option<ZwpKeyboardShortcutsInhibitManagerV1>,
    pub foreign_toplevel_list: Option<ExtForeignToplevelListV1>,
    pub output_manager: Option<ZwlrOutputManagerV1>,
    pub security_context_manager: Option<WpSecurityContextManagerV1>,

    pub windows: Vec<Window>,
    pub layers: Vec<LayerSurface>,
    pub foreign_toplevels: Vec<ForeignToplevel>,
    pub output_heads: Vec<OutputHead>,
    pub output_modes: HashMap<ZwlrOutputModeV1, OutputMode>,
    /// The serial of the last wlr-output-management `done` event.
    pub output_manager_serial: u32,
    pub output_configurations: HashMap<ZwlrOutputConfigurationV1, OutputConfigurationResult>,
}

/// A toplevel announced through ext-foreign-toplevel-list.
#[derive(Debug)]
pub struct ForeignToplevel {
    pub handle: ExtForeignToplevelHandleV1,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub closed: bool,
}

/// A head announced through wlr-output-management.
#[derive(Debug)]
pub struct OutputHead {
    pub head: ZwlrOutputHeadV1,
    /// The manager that announced this head.
    pub manager: ZwlrOutputManagerV1,
    pub name: Option<String>,
    pub enabled: bool,
    pub modes: Vec<ZwlrOutputModeV1>,
    pub current_mode: Option<ZwlrOutputModeV1>,
}

/// How the compositor answered a wlr-output-management configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputConfigurationResult {
    Succeeded,
    Failed,
    Cancelled,
}

/// A mode announced through wlr-output-management.
#[derive(Debug, Default)]
pub struct OutputMode {
    pub size: (i32, i32),
    pub refresh: i32,
    pub preferred: bool,
}

pub struct Window {
    pub qh: QueueHandle<State>,
    pub spbm: WpSinglePixelBufferManagerV1,

    pub surface: WlSurface,
    pub xdg_surface: XdgSurface,
    pub xdg_toplevel: XdgToplevel,
    pub viewport: WpViewport,
    pub pending_configure: Configure,
    pub configures_received: Vec<(u32, Configure)>,
    pub close_requested: bool,

    pub configures_looked_at: usize,
}

pub struct LayerSurface {
    pub qh: QueueHandle<State>,
    pub spbm: WpSinglePixelBufferManagerV1,

    pub surface: WlSurface,
    pub layer_surface: ZwlrLayerSurfaceV1,
    pub viewport: WpViewport,
    pub configures_received: Vec<(u32, LayerConfigure)>,
    pub close_requested: bool,

    pub configures_looked_at: usize,
}

#[derive(Debug, Default)]
pub struct Seat {
    pub keyboard: Option<Keyboard>,
}

#[derive(Debug)]
pub struct Surface {
    pub keyboard_events_received: Vec<KeyboardEvent>,
    pub keyboard_events_looked_at: usize,
    /// Outputs from `wl_surface.enter` without a matching `wl_surface.leave`.
    pub entered_outputs: Vec<WlOutput>,
    /// The last `wl_surface.preferred_buffer_scale`.
    pub preferred_buffer_scale: Option<i32>,
}

/// User data of a popup's `xdg_surface`, whose configures are acked right away.
pub struct PopupXdgSurface;

#[derive(Debug)]
pub struct Keyboard {
    pub proxy: WlKeyboard,
    pub surface: Option<WlSurface>,
}

#[derive(Debug)]
pub enum KeyboardEvent {
    Enter {
        keys: Vec<u32>,
    },
    Leave,
    Key {
        key: u32,
        state: wl_keyboard::KeyState,
    },
    Modifiers {
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Configure {
    pub size: (i32, i32),
    pub bounds: Option<(i32, i32)>,
    pub states: Vec<xdg_toplevel::State>,
}

#[derive(Debug, Clone, Copy)]
pub struct LayerConfigure {
    pub size: (u32, u32),
}

#[derive(Clone, Copy, Default)]
pub struct LayerMargin {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

#[derive(Clone, Copy, Default)]
pub struct LayerConfigureProps {
    pub size: Option<(u32, u32)>,
    pub anchor: Option<zwlr_layer_surface_v1::Anchor>,
    pub exclusive_zone: Option<i32>,
    pub margin: Option<LayerMargin>,
    pub kb_interactivity: Option<zwlr_layer_surface_v1::KeyboardInteractivity>,
    pub layer: Option<zwlr_layer_shell_v1::Layer>,
    pub exclusive_edge: Option<zwlr_layer_surface_v1::Anchor>,
}

#[derive(Default)]
pub struct SyncData {
    pub done: AtomicBool,
}

/// What an ext-session-lock lock object has received.
#[derive(Debug, Default)]
pub struct SessionLockData {
    pub locked: AtomicBool,
    pub finished: AtomicBool,
}

static CLIENT_ID_COUNTER: IdCounter = IdCounter::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(u64);

impl ClientId {
    fn next() -> ClientId {
        ClientId(CLIENT_ID_COUNTER.next())
    }
}

impl fmt::Display for KeyboardEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyboardEvent::Enter { keys } => {
                write!(f, "enter: {keys:?}")?;
            }
            KeyboardEvent::Leave => {
                write!(f, "leave")?;
            }
            KeyboardEvent::Key { key, state } => {
                let action = match state {
                    wl_keyboard::KeyState::Released => "released",
                    wl_keyboard::KeyState::Pressed => "pressed",
                    wl_keyboard::KeyState::Repeated => "repeated",
                    _ => unreachable!(),
                };
                write!(f, "key {action}: {key}")?;
            }
            KeyboardEvent::Modifiers {
                depressed,
                latched,
                locked,
                group,
            } => {
                write!(
                    f,
                    "modifiers: depressed={depressed}, latched={latched}, \
                     locked={locked}, group={group}"
                )?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Configure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "size: {} × {}, ", self.size.0, self.size.1)?;
        if let Some(bounds) = self.bounds {
            write!(f, "bounds: {} × {}, ", bounds.0, bounds.1)?;
        } else {
            write!(f, "bounds: none, ")?;
        }
        write!(f, "states: {:?}", self.states)?;
        Ok(())
    }
}

impl fmt::Display for LayerConfigure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "size: {} × {}", self.size.0, self.size.1)?;
        Ok(())
    }
}

impl Client {
    pub fn new(stream: UnixStream) -> Self {
        let id = ClientId::next();

        let event_loop = EventLoop::try_new().unwrap();
        let backend = Backend::connect(stream).unwrap();
        let connection = Connection::from_backend(backend);
        let queue = connection.new_event_queue();
        let qh = queue.handle();
        WaylandSource::new(connection.clone(), queue)
            .insert(event_loop.handle())
            .unwrap();

        let display = connection.display();
        let registry = display.get_registry(&qh, ());
        connection.flush().unwrap();

        let state = State {
            qh: qh.clone(),
            globals: Vec::new(),
            outputs: HashMap::new(),
            surfaces: HashMap::new(),
            seats: HashMap::new(),
            compositor: None,
            subcompositor: None,
            xdg_wm_base: None,
            layer_shell: None,
            session_lock_manager: None,
            virtual_pointer_manager: None,
            virtual_pointer: None,
            spbm: None,
            shm: None,
            text_input_manager: None,
            input_method_manager: None,
            viewporter: None,
            ksim: None,
            foreign_toplevel_list: None,
            output_manager: None,
            security_context_manager: None,
            windows: Vec::new(),
            layers: Vec::new(),
            foreign_toplevels: Vec::new(),
            output_heads: Vec::new(),
            output_modes: HashMap::new(),
            output_manager_serial: 0,
            output_configurations: HashMap::new(),
        };

        Self {
            id,
            event_loop,
            connection,
            qh,
            display,
            registry,
            state,
            expect_protocol_error: false,
        }
    }

    pub fn dispatch(&mut self) {
        let res = self.event_loop.dispatch(Duration::ZERO, &mut self.state);

        if let Some(error) = self.connection.protocol_error() {
            if self.expect_protocol_error {
                return;
            }
            panic!("{error}");
        }

        res.unwrap();
    }

    pub fn send_sync(&self) -> Arc<SyncData> {
        let data = Arc::new(SyncData::default());
        self.display.sync(&self.qh, data.clone());
        self.connection.flush().unwrap();
        data
    }

    /// Binds wlr-output-management once more, as another part of the same client would.
    pub fn bind_output_manager(&mut self) -> ZwlrOutputManagerV1 {
        let global = self
            .state
            .globals
            .iter()
            .find(|g| g.interface == ZwlrOutputManagerV1::interface().name)
            .unwrap();
        let version = min(global.version, ZwlrOutputManagerV1::interface().version);
        self.registry.bind(global.name, version, &self.qh, ())
    }

    pub fn create_window(&mut self) -> &mut Window {
        self.state.create_window()
    }

    pub fn window(&mut self, surface: &WlSurface) -> &mut Window {
        self.state.window(surface)
    }

    pub fn create_layer(
        &mut self,
        output: Option<&WlOutput>,
        layer: zwlr_layer_shell_v1::Layer,
        namespace: &str,
    ) -> &mut LayerSurface {
        self.state.create_layer(output, layer, namespace.to_owned())
    }

    pub fn layer(&mut self, surface: &WlSurface) -> &mut LayerSurface {
        self.state.layer(surface)
    }

    pub fn output(&mut self, name: &str) -> WlOutput {
        self.state
            .outputs
            .iter()
            .find(|(_, v)| *v == name)
            .unwrap()
            .0
            .clone()
    }
}

impl Client {
    /// Locks the session with a lock surface on every output.
    ///
    /// The lock surfaces get a buffer of the configured size on every configure.
    pub fn lock_session(&mut self) -> (ExtSessionLockV1, Arc<SessionLockData>) {
        let data = Arc::new(SessionLockData::default());
        let qh = self.state.qh.clone();
        let manager = self.state.session_lock_manager.as_ref().unwrap();
        let lock = manager.lock(&qh, data.clone());

        let outputs: Vec<_> = self.state.outputs.keys().cloned().collect();
        for output in outputs {
            let surface = self.state.create_surface();
            let viewport = self.state.viewporter.as_ref().unwrap();
            let viewport = viewport.get_viewport(&surface, &qh, ());
            lock.get_lock_surface(&surface, &output, &qh, (surface.clone(), viewport));
        }

        (lock, data)
    }
}

impl State {
    pub fn create_surface(&mut self) -> WlSurface {
        let compositor = self.compositor.as_ref().unwrap();
        let surface = compositor.create_surface(&self.qh, ());
        self.surfaces.insert(
            surface.clone(),
            Surface {
                keyboard_events_received: Vec::new(),
                keyboard_events_looked_at: 0,
                entered_outputs: Vec::new(),
                preferred_buffer_scale: None,
            },
        );
        surface
    }

    /// Attaches a 1×1 buffer to the surface, without committing.
    pub fn attach_pixel_buffer(&self, surface: &WlSurface) {
        let spbm = self.spbm.as_ref().unwrap();
        let buffer = spbm.create_u32_rgba_buffer(0, 0, 0, u32::MAX, &self.qh, ());
        surface.attach(Some(&buffer), 0, 0);
    }

    /// Attaches an opaque `width` × `height` buffer of one RGB color, without committing.
    ///
    /// Unlike single-pixel buffers, these are uploaded to a texture, so they show up in snapshots.
    pub fn attach_shm_buffer(&self, surface: &WlSurface, width: i32, height: i32, rgb: [u8; 3]) {
        let [r, g, b] = rgb;
        let stride = width * 4;
        let fd = memfd_create("nori-test-buffer", MemfdFlags::CLOEXEC).unwrap();
        let mut file = File::from(fd);
        // Xrgb8888 is stored as B, G, R, X.
        let pixels = [b, g, r, 255].repeat((width * height) as usize);
        file.write_all(&pixels).unwrap();

        let shm = self.shm.as_ref().unwrap();
        let pool = shm.create_pool(file.as_fd(), stride * height, &self.qh, ());
        let format = wl_shm::Format::Xrgb8888;
        let buffer = pool.create_buffer(0, width, height, stride, format, &self.qh, ());
        pool.destroy();

        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
    }

    /// Creates a synchronized subsurface with a buffer committed; it appears on the parent's next
    /// commit.
    pub fn create_subsurface(&mut self, parent: &WlSurface) -> (WlSurface, WlSubsurface) {
        let surface = self.create_surface();
        let subcompositor = self.subcompositor.as_ref().unwrap();
        let subsurface = subcompositor.get_subsurface(&surface, parent, &self.qh, ());
        self.attach_pixel_buffer(&surface);
        surface.commit();
        (surface, subsurface)
    }

    /// Creates a 10×10 popup at the parent's top-left corner, placed towards `gravity`, and sends
    /// its initial commit; it maps when committed with a buffer after the configure.
    pub fn create_popup(
        &mut self,
        parent: &XdgSurface,
        gravity: xdg_positioner::Gravity,
    ) -> (WlSurface, XdgSurface, XdgPopup) {
        self.create_popup_with(Some(parent), gravity, |_| (), |_| ())
    }

    /// Creates a `width` × `height` popup at (`x`, `y`) from the parent's top-left corner, and
    /// sends its initial commit; it maps when committed with a buffer after the configure.
    pub fn create_popup_at(
        &mut self,
        parent: &XdgSurface,
        (x, y): (i32, i32),
        (width, height): (i32, i32),
    ) -> (WlSurface, XdgSurface, XdgPopup) {
        let place = |positioner: &XdgPositioner| {
            positioner.set_size(width, height);
            positioner.set_anchor_rect(x, y, 1, 1);
        };
        self.create_popup_with(
            Some(parent),
            xdg_positioner::Gravity::BottomRight,
            place,
            |_| (),
        )
    }

    /// Like [`Self::create_popup()`], for a layer surface parent.
    pub fn create_layer_popup(
        &mut self,
        parent: &ZwlrLayerSurfaceV1,
        gravity: xdg_positioner::Gravity,
    ) -> (WlSurface, XdgSurface, XdgPopup) {
        self.create_popup_with(None, gravity, |_| (), |popup| parent.get_popup(popup))
    }

    fn create_popup_with(
        &mut self,
        parent: Option<&XdgSurface>,
        gravity: xdg_positioner::Gravity,
        place: impl FnOnce(&XdgPositioner),
        set_parent: impl FnOnce(&XdgPopup),
    ) -> (WlSurface, XdgSurface, XdgPopup) {
        let surface = self.create_surface();
        let xdg_wm_base = self.xdg_wm_base.as_ref().unwrap();
        let positioner = xdg_wm_base.create_positioner(&self.qh, ());
        positioner.set_size(10, 10);
        positioner.set_anchor_rect(0, 0, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(gravity);
        place(&positioner);
        let xdg_surface = xdg_wm_base.get_xdg_surface(&surface, &self.qh, PopupXdgSurface);
        let popup = xdg_surface.get_popup(parent, &positioner, &self.qh, ());
        set_parent(&popup);
        positioner.destroy();
        surface.commit();
        (surface, xdg_surface, popup)
    }

    pub fn create_window(&mut self) -> &mut Window {
        let surface = self.create_surface();

        let xdg_wm_base = self.xdg_wm_base.as_ref().unwrap();
        let viewporter = self.viewporter.as_ref().unwrap();

        let xdg_surface = xdg_wm_base.get_xdg_surface(&surface, &self.qh, ());
        let xdg_toplevel = xdg_surface.get_toplevel(&self.qh, ());
        let viewport = viewporter.get_viewport(&surface, &self.qh, ());

        let window = Window {
            qh: self.qh.clone(),
            spbm: self.spbm.clone().unwrap(),

            surface,
            xdg_surface,
            xdg_toplevel,
            viewport,
            pending_configure: Configure::default(),
            configures_received: Vec::new(),
            close_requested: false,

            configures_looked_at: 0,
        };

        self.windows.push(window);
        self.windows.last_mut().unwrap()
    }

    pub fn window(&mut self, surface: &WlSurface) -> &mut Window {
        self.windows
            .iter_mut()
            .find(|w| w.surface == *surface)
            .unwrap()
    }

    pub fn create_layer(
        &mut self,
        output: Option<&WlOutput>,
        layer: zwlr_layer_shell_v1::Layer,
        namespace: String,
    ) -> &mut LayerSurface {
        let surface = self.create_surface();

        let layer_shell = self.layer_shell.as_ref().unwrap();
        let viewporter = self.viewporter.as_ref().unwrap();

        let layer_surface =
            layer_shell.get_layer_surface(&surface, output, layer, namespace, &self.qh, ());
        let viewport = viewporter.get_viewport(&surface, &self.qh, ());

        let layer_surface = LayerSurface {
            qh: self.qh.clone(),
            spbm: self.spbm.clone().unwrap(),

            surface,
            layer_surface,
            viewport,
            configures_received: Vec::new(),
            close_requested: false,

            configures_looked_at: 0,
        };

        self.layers.push(layer_surface);
        self.layers.last_mut().unwrap()
    }

    pub fn layer(&mut self, surface: &WlSurface) -> &mut LayerSurface {
        self.layers
            .iter_mut()
            .find(|w| w.surface == *surface)
            .unwrap()
    }

    pub fn recent_keyboard_events(
        &mut self,
        surface: &WlSurface,
    ) -> impl Iterator<Item = &KeyboardEvent> + '_ {
        let surface = self.surfaces.get_mut(surface).unwrap();
        surface.recent_keyboard_events()
    }

    pub fn inhibit_shortcuts(&self, surface: &WlSurface) -> ZwpKeyboardShortcutsInhibitorV1 {
        let ksim = self.ksim.as_ref().unwrap();
        let seat = self.seats.keys().next().unwrap();
        ksim.inhibit_shortcuts(surface, seat, &self.qh, ())
    }

    pub fn virtual_pointer(&mut self) -> ZwlrVirtualPointerV1 {
        if let Some(pointer) = &self.virtual_pointer {
            return pointer.clone();
        }

        let manager = self.virtual_pointer_manager.as_ref().unwrap();
        let pointer = manager.create_virtual_pointer(None, &self.qh, ());
        self.virtual_pointer = Some(pointer.clone());
        pointer
    }
}

impl Window {
    pub fn commit(&self) {
        self.surface.commit();
    }

    pub fn ack_last(&self) {
        let serial = self.configures_received.last().unwrap().0;
        self.xdg_surface.ack_configure(serial);
    }

    pub fn ack_last_and_commit(&self) {
        self.ack_last();
        self.commit();
    }

    pub fn attach_new_buffer(&self) {
        let buffer = self.spbm.create_u32_rgba_buffer(0, 0, 0, 0, &self.qh, ());
        self.surface.attach(Some(&buffer), 0, 0);
    }

    pub fn attach_null(&self) {
        self.surface.attach(None, 0, 0);
    }

    pub fn set_size(&self, w: u16, h: u16) {
        self.viewport.set_destination(i32::from(w), i32::from(h));
    }

    pub fn set_fullscreen(&self, output: Option<&WlOutput>) {
        self.xdg_toplevel.set_fullscreen(output);
    }

    pub fn unset_fullscreen(&self) {
        self.xdg_toplevel.unset_fullscreen();
    }

    pub fn set_maximized(&self) {
        self.xdg_toplevel.set_maximized();
    }

    pub fn unset_maximized(&self) {
        self.xdg_toplevel.unset_maximized();
    }

    pub fn set_parent(&self, parent: Option<&XdgToplevel>) {
        self.xdg_toplevel.set_parent(parent);
    }

    pub fn set_title(&self, title: &str) {
        self.xdg_toplevel.set_title(title.to_owned());
    }

    pub fn recent_configures(&mut self) -> impl Iterator<Item = &Configure> {
        let start = self.configures_looked_at;
        self.configures_looked_at = self.configures_received.len();
        self.configures_received[start..].iter().map(|(_, c)| c)
    }

    pub fn format_recent_configures(&mut self) -> String {
        let mut buf = String::new();
        for configure in self.recent_configures() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            write!(buf, "{configure}").unwrap();
        }
        buf
    }
}

impl LayerSurface {
    pub fn commit(&self) {
        self.surface.commit();
    }

    pub fn ack_last(&self) {
        let serial = self.configures_received.last().unwrap().0;
        self.layer_surface.ack_configure(serial);
    }

    pub fn ack_last_and_commit(&self) {
        self.ack_last();
        self.commit();
    }

    pub fn set_configure_props(&self, props: LayerConfigureProps) {
        let LayerConfigureProps {
            size,
            anchor,
            exclusive_zone,
            margin,
            kb_interactivity,
            layer,
            exclusive_edge,
        } = props;

        if let Some(x) = size {
            self.layer_surface.set_size(x.0, x.1);
        }
        if let Some(x) = anchor {
            self.layer_surface.set_anchor(x);
        }
        if let Some(x) = exclusive_zone {
            self.layer_surface.set_exclusive_zone(x);
        }
        if let Some(x) = margin {
            self.layer_surface
                .set_margin(x.top, x.right, x.bottom, x.left);
        }
        if let Some(x) = kb_interactivity {
            self.layer_surface.set_keyboard_interactivity(x);
        }
        if let Some(x) = layer {
            self.layer_surface.set_layer(x);
        }
        if let Some(x) = exclusive_edge {
            self.layer_surface.set_exclusive_edge(x);
        }
    }

    pub fn attach_new_buffer(&self) {
        let buffer = self.spbm.create_u32_rgba_buffer(0, 0, 0, 0, &self.qh, ());
        self.surface.attach(Some(&buffer), 0, 0);
    }

    pub fn attach_null(&self) {
        self.surface.attach(None, 0, 0);
    }

    pub fn set_size(&self, w: u16, h: u16) {
        self.viewport.set_destination(i32::from(w), i32::from(h));
    }

    pub fn recent_configures(&mut self) -> impl Iterator<Item = &LayerConfigure> {
        let start = self.configures_looked_at;
        self.configures_looked_at = self.configures_received.len();
        self.configures_received[start..].iter().map(|(_, c)| c)
    }

    pub fn format_recent_configures(&mut self) -> String {
        let mut buf = String::new();
        for configure in self.recent_configures() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            write!(buf, "{configure}").unwrap();
        }
        buf
    }
}

impl Surface {
    pub fn recent_keyboard_events(&mut self) -> impl Iterator<Item = &KeyboardEvent> {
        let start = self.keyboard_events_looked_at;
        self.keyboard_events_looked_at = self.keyboard_events_received.len();
        self.keyboard_events_received[start..].iter()
    }
}

impl Dispatch<WlCallback, Arc<SyncData>> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlCallback,
        event: <WlCallback as wayland_client::Proxy>::Event,
        data: &Arc<SyncData>,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_callback::Event::Done { .. } => data.done.store(true, Ordering::Relaxed),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface == WlCompositor::interface().name {
                    let version = min(version, WlCompositor::interface().version);
                    state.compositor = Some(registry.bind(name, version, qh, ()));
                } else if interface == XdgWmBase::interface().name {
                    let version = min(version, XdgWmBase::interface().version);
                    state.xdg_wm_base = Some(registry.bind(name, version, qh, ()));
                } else if interface == ZwlrLayerShellV1::interface().name {
                    let version = min(version, ZwlrLayerShellV1::interface().version);
                    state.layer_shell = Some(registry.bind(name, version, qh, ()));
                } else if interface == ExtSessionLockManagerV1::interface().name {
                    let version = min(version, ExtSessionLockManagerV1::interface().version);
                    state.session_lock_manager = Some(registry.bind(name, version, qh, ()));
                } else if interface == ZwlrVirtualPointerManagerV1::interface().name {
                    let version = min(version, ZwlrVirtualPointerManagerV1::interface().version);
                    state.virtual_pointer_manager = Some(registry.bind(name, version, qh, ()));
                } else if interface == WpSinglePixelBufferManagerV1::interface().name {
                    let version = min(version, WpSinglePixelBufferManagerV1::interface().version);
                    state.spbm = Some(registry.bind(name, version, qh, ()));
                } else if interface == WpViewporter::interface().name {
                    let version = min(version, WpViewporter::interface().version);
                    state.viewporter = Some(registry.bind(name, version, qh, ()));
                } else if interface == WlOutput::interface().name {
                    let version = min(version, WlOutput::interface().version);
                    let output = registry.bind(name, version, qh, ());
                    state.outputs.insert(output, String::new());
                } else if interface == WlShm::interface().name {
                    let version = min(version, WlShm::interface().version);
                    state.shm = Some(registry.bind(name, version, qh, ()));
                } else if interface == ZwpTextInputManagerV3::interface().name {
                    let version = min(version, ZwpTextInputManagerV3::interface().version);
                    state.text_input_manager = Some(registry.bind(name, version, qh, ()));
                } else if interface == ZwpInputMethodManagerV2::interface().name {
                    let version = min(version, ZwpInputMethodManagerV2::interface().version);
                    state.input_method_manager = Some(registry.bind(name, version, qh, ()));
                } else if interface == WlSubcompositor::interface().name {
                    let version = min(version, WlSubcompositor::interface().version);
                    state.subcompositor = Some(registry.bind(name, version, qh, ()));
                } else if interface == WlSeat::interface().name {
                    let version = min(version, WlSeat::interface().version);
                    state
                        .seats
                        .insert(registry.bind(name, version, qh, ()), Seat::default());
                } else if interface == ZwpKeyboardShortcutsInhibitManagerV1::interface().name {
                    let version = min(
                        version,
                        ZwpKeyboardShortcutsInhibitManagerV1::interface().version,
                    );
                    state.ksim = Some(registry.bind(name, version, qh, ()));
                } else if interface == ExtForeignToplevelListV1::interface().name {
                    let version = min(version, ExtForeignToplevelListV1::interface().version);
                    state.foreign_toplevel_list = Some(registry.bind(name, version, qh, ()));
                } else if interface == ZwlrOutputManagerV1::interface().name {
                    let version = min(version, ZwlrOutputManagerV1::interface().version);
                    state.output_manager = Some(registry.bind(name, version, qh, ()));
                } else if interface == WpSecurityContextManagerV1::interface().name {
                    let version = min(version, WpSecurityContextManagerV1::interface().version);
                    state.security_context_manager = Some(registry.bind(name, version, qh, ()));
                }

                let global = Global {
                    name,
                    interface,
                    version,
                };
                state.globals.push(global);
            }
            wl_registry::Event::GlobalRemove { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: <WlOutput as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Geometry { .. } => (),
            wl_output::Event::Mode { .. } => (),
            wl_output::Event::Done => (),
            wl_output::Event::Scale { .. } => (),
            wl_output::Event::Name { name } => {
                *state.outputs.get_mut(output).unwrap() = name;
            }
            wl_output::Event::Description { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: <WlSeat as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Capabilities { capabilities } => {
                let capabilities = capabilities.into_result().unwrap();
                let data = state.seats.get_mut(seat).unwrap();

                if capabilities.contains(wl_seat::Capability::Keyboard) {
                    if data.keyboard.is_none() {
                        let keyboard = Keyboard {
                            proxy: seat.get_keyboard(qh, seat.clone()),
                            surface: None,
                        };
                        data.keyboard = Some(keyboard);
                    }
                } else {
                    if let Some(keyboard) = data.keyboard.take() {
                        keyboard.proxy.release();
                    }
                }
            }
            wl_seat::Event::Name { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlKeyboard, WlSeat> for State {
    fn event(
        state: &mut Self,
        _keyboard: &WlKeyboard,
        event: <WlKeyboard as wayland_client::Proxy>::Event,
        seat: &WlSeat,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let seat = state.seats.get_mut(seat).unwrap();
        let keyboard = seat.keyboard.as_mut().unwrap();

        match event {
            wl_keyboard::Event::Keymap { .. } => (),
            wl_keyboard::Event::Enter { surface, keys, .. } => {
                keyboard.surface = Some(surface.clone());

                // The order is arbitrary, but we need a fixed order for our tests.
                let mut keys: Vec<u32> = bytemuck::pod_collect_to_vec(&keys);
                keys.sort_unstable();

                let event = KeyboardEvent::Enter { keys };
                let surface = state.surfaces.get_mut(&surface).unwrap();
                surface.keyboard_events_received.push(event);
            }
            wl_keyboard::Event::Leave { .. } => {
                let surface = keyboard.surface.take().unwrap();

                let event = KeyboardEvent::Leave;
                let surface = state.surfaces.get_mut(&surface).unwrap();
                surface.keyboard_events_received.push(event);
            }
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                let key_state = key_state.into_result().unwrap();
                let surface = keyboard.surface.clone().unwrap();

                let event = KeyboardEvent::Key {
                    key,
                    state: key_state,
                };
                let surface = state.surfaces.get_mut(&surface).unwrap();
                surface.keyboard_events_received.push(event);
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                let surface = keyboard.surface.clone().unwrap();

                let event = KeyboardEvent::Modifiers {
                    depressed: mods_depressed,
                    latched: mods_latched,
                    locked: mods_locked,
                    group,
                };
                let surface = state.surfaces.get_mut(&surface).unwrap();
                surface.keyboard_events_received.push(event);
            }
            wl_keyboard::Event::RepeatInfo { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlCompositor, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _state: &mut Self,
        xdg_wm_base: &XdgWmBase,
        event: <XdgWmBase as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_wm_base::Event::Ping { serial } => {
                xdg_wm_base.pong(serial);
            }
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrLayerShellV1,
        _event: <ZwlrLayerShellV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<ExtSessionLockV1, Arc<SessionLockData>> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ExtSessionLockV1,
        event: <ExtSessionLockV1 as wayland_client::Proxy>::Event,
        data: &Arc<SessionLockData>,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => data.locked.store(true, Ordering::Relaxed),
            ext_session_lock_v1::Event::Finished => data.finished.store(true, Ordering::Relaxed),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, (WlSurface, WpViewport)> for State {
    fn event(
        state: &mut Self,
        lock_surface: &ExtSessionLockSurfaceV1,
        event: <ExtSessionLockSurfaceV1 as wayland_client::Proxy>::Event,
        (surface, viewport): &(WlSurface, WpViewport),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                let spbm = state.spbm.as_ref().unwrap();
                let buffer = spbm.create_u32_rgba_buffer(0, 0, 0, u32::MAX, qh, ());
                surface.attach(Some(&buffer), 0, 0);
                viewport.set_destination(width as i32, height as i32);
                lock_surface.ack_configure(serial);
                surface.commit();
            }
            _ => unreachable!(),
        }
    }
}

wayland_client::delegate_noop!(State: ExtSessionLockManagerV1);
wayland_client::delegate_noop!(State: ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(State: WlRegion);
wayland_client::delegate_noop!(State: ignore WlSubcompositor);
wayland_client::delegate_noop!(State: ignore WlSubsurface);
wayland_client::delegate_noop!(State: ignore XdgPositioner);
wayland_client::delegate_noop!(State: ZwlrVirtualPointerV1);
wayland_client::delegate_noop!(State: WpSecurityContextManagerV1);
wayland_client::delegate_noop!(State: WpSecurityContextV1);

impl Dispatch<WlSurface, ()> for State {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: <WlSurface as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { output } => {
                if let Some(surface) = state.surfaces.get_mut(surface) {
                    surface.entered_outputs.push(output);
                }
            }
            wl_surface::Event::Leave { output } => {
                if let Some(surface) = state.surfaces.get_mut(surface) {
                    surface.entered_outputs.retain(|o| *o != output);
                }
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                if let Some(surface) = state.surfaces.get_mut(surface) {
                    surface.preferred_buffer_scale = Some(factor);
                }
            }
            wl_surface::Event::PreferredBufferTransform { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: <XdgSurface as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_surface::Event::Configure { serial } => {
                let window = state
                    .windows
                    .iter_mut()
                    .find(|w| w.xdg_surface == *xdg_surface)
                    .unwrap();
                let configure = window.pending_configure.clone();
                window.configures_received.push((serial, configure));
            }
            _ => unreachable!(),
        }
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        xdg_toplevel: &XdgToplevel,
        event: <XdgToplevel as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let window = state
            .windows
            .iter_mut()
            .find(|w| w.xdg_toplevel == *xdg_toplevel)
            .unwrap();

        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let configure = &mut window.pending_configure;
                configure.size = (width, height);
                configure.states = states
                    .chunks_exact(4)
                    .flat_map(TryInto::<[u8; 4]>::try_into)
                    .map(u32::from_ne_bytes)
                    .flat_map(xdg_toplevel::State::try_from)
                    .collect();
            }
            xdg_toplevel::Event::Close => {
                window.close_requested = true;
            }
            xdg_toplevel::Event::ConfigureBounds { width, height } => {
                window.pending_configure.bounds = Some((width, height));
            }
            xdg_toplevel::Event::WmCapabilities { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let layer_surface = state
            .layers
            .iter_mut()
            .find(|w| w.layer_surface == *layer_surface)
            .unwrap();

        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                let configure = LayerConfigure {
                    size: (width, height),
                };
                layer_surface.configures_received.push((serial, configure));
            }
            zwlr_layer_surface_v1::Event::Closed => layer_surface.close_requested = true,
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        event: <WlBuffer as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_buffer::Event::Release => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlShm, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlShm,
        event: <WlShm as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_shm::Event::Format { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<WlShmPool, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlShmPool,
        _event: <WlShmPool as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

macro_rules! ignore_events {
    ($($proxy:ty),*) => {$(
        impl Dispatch<$proxy, ()> for State {
            fn event(
                _state: &mut Self,
                _proxy: &$proxy,
                _event: <$proxy as wayland_client::Proxy>::Event,
                _data: &(),
                _conn: &Connection,
                _qhandle: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

// The input method tests only look at which outputs the popup surface entered.
ignore_events!(
    ZwpTextInputManagerV3,
    ZwpTextInputV3,
    ZwpInputMethodManagerV2,
    ZwpInputMethodV2,
    ZwpInputPopupSurfaceV2
);

impl Dispatch<WpSinglePixelBufferManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpSinglePixelBufferManagerV1,
        _event: <WpSinglePixelBufferManagerV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<WpViewporter, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        _event: <WpViewporter as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<WpViewport, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        _event: <WpViewport as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpKeyboardShortcutsInhibitManagerV1,
        _event: <ZwpKeyboardShortcutsInhibitManagerV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        unreachable!()
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitorV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpKeyboardShortcutsInhibitorV1,
        event: <ZwpKeyboardShortcutsInhibitorV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Active => (),
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<XdgSurface, PopupXdgSurface> for State {
    fn event(
        _state: &mut Self,
        xdg_surface: &XdgSurface,
        event: <XdgSurface as wayland_client::Proxy>::Event,
        _data: &PopupXdgSurface,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_surface::Event::Configure { serial } => xdg_surface.ack_configure(serial),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<XdgPopup, ()> for State {
    fn event(
        _state: &mut Self,
        _popup: &XdgPopup,
        event: <XdgPopup as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure { .. } => (),
            xdg_popup::Event::PopupDone => (),
            xdg_popup::Event::Repositioned { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &ExtForeignToplevelListV1,
        event: <ExtForeignToplevelListV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } => {
                state.foreign_toplevels.push(ForeignToplevel {
                    handle: toplevel,
                    title: None,
                    app_id: None,
                    closed: false,
                });
            }
            ext_foreign_toplevel_list_v1::Event::Finished => (),
            _ => unreachable!(),
        }
    }

    wayland_client::event_created_child!(State, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: <ExtForeignToplevelHandleV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let toplevel = state
            .foreign_toplevels
            .iter_mut()
            .find(|t| t.handle == *handle)
            .unwrap();

        match event {
            ext_foreign_toplevel_handle_v1::Event::Closed => toplevel.closed = true,
            ext_foreign_toplevel_handle_v1::Event::Done => (),
            ext_foreign_toplevel_handle_v1::Event::Title { title } => toplevel.title = Some(title),
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                toplevel.app_id = Some(app_id)
            }
            ext_foreign_toplevel_handle_v1::Event::Identifier { .. } => (),
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        manager: &ZwlrOutputManagerV1,
        event: <ZwlrOutputManagerV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.output_heads.push(OutputHead {
                    head,
                    manager: manager.clone(),
                    name: None,
                    enabled: false,
                    modes: Vec::new(),
                    current_mode: None,
                });
            }
            zwlr_output_manager_v1::Event::Done { serial } => state.output_manager_serial = serial,
            zwlr_output_manager_v1::Event::Finished => (),
            _ => unreachable!(),
        }
    }

    wayland_client::event_created_child!(State, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for State {
    fn event(
        state: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: <ZwlrOutputHeadV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let output_head = state
            .output_heads
            .iter_mut()
            .find(|h| h.head == *head)
            .unwrap();

        match event {
            zwlr_output_head_v1::Event::Name { name } => output_head.name = Some(name),
            zwlr_output_head_v1::Event::Mode { mode } => {
                state
                    .output_modes
                    .insert(mode.clone(), OutputMode::default());
                output_head.modes.push(mode);
            }
            zwlr_output_head_v1::Event::Enabled { enabled } => output_head.enabled = enabled != 0,
            zwlr_output_head_v1::Event::CurrentMode { mode } => {
                output_head.current_mode = Some(mode)
            }
            // Description, position, scale and the other properties aren't needed yet.
            _ => (),
        }
    }

    wayland_client::event_created_child!(State, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for State {
    fn event(
        state: &mut Self,
        mode: &ZwlrOutputModeV1,
        event: <ZwlrOutputModeV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let output_mode = state.output_modes.get_mut(mode).unwrap();

        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => {
                output_mode.size = (width, height)
            }
            zwlr_output_mode_v1::Event::Refresh { refresh } => output_mode.refresh = refresh,
            zwlr_output_mode_v1::Event::Preferred => output_mode.preferred = true,
            zwlr_output_mode_v1::Event::Finished => {
                state.output_modes.remove(mode);
            }
            _ => unreachable!(),
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for State {
    fn event(
        state: &mut Self,
        conf: &ZwlrOutputConfigurationV1,
        event: <ZwlrOutputConfigurationV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let result = match event {
            zwlr_output_configuration_v1::Event::Succeeded => OutputConfigurationResult::Succeeded,
            zwlr_output_configuration_v1::Event::Failed => OutputConfigurationResult::Failed,
            zwlr_output_configuration_v1::Event::Cancelled => OutputConfigurationResult::Cancelled,
            _ => unreachable!(),
        };
        state.output_configurations.insert(conf.clone(), result);
    }
}

wayland_client::delegate_noop!(State: ZwlrOutputConfigurationHeadV1);
