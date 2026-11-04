//! Headless backend for tests.
//!
//! This can eventually grow into a more complete backend if needed, but for now it's missing some
//! crucial parts like dmabufs.

use std::mem;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context as _;
use niri_config::OutputName;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::drm::DrmNode;
use smithay::backend::egl::native::EGLSurfacelessDisplay;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::utils::Size;
use smithay::wayland::presentation::Refresh;

use super::{IpcOutputMap, OutputId, RenderResult};
use crate::niri::{Niri, RedrawState};
use crate::render_helpers::{resources, shaders};
use crate::utils::{get_monotonic_time, logical_output};

pub struct Headless {
    renderer: Option<GlesRenderer>,
    ipc_outputs: Arc<Mutex<IpcOutputMap>>,
}

/// Creates a surfaceless EGL display, keeping one handle to it alive until the process exits.
///
/// Every surfaceless `EGLDisplay` is the same EGL display, and smithay calls `eglTerminate` when
/// the last handle to it is dropped, without taking the lock that `EGLDisplay::new` takes. So when
/// one test dropped its last renderer while another test was in `EGLDisplay::new`, the display
/// could be terminated under the second test (#263). With a handle that is never dropped,
/// `eglTerminate` is never called. Creating displays under `KEPT` makes sure that the first one is
/// kept before another test can get the same display.
///
/// # Safety
///
/// Same as `EGLDisplay::new`.
unsafe fn surfaceless_egl_display() -> anyhow::Result<EGLDisplay> {
    static KEPT: Mutex<Option<EGLDisplay>> = Mutex::new(None);

    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let display =
        unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }.context("error creating EGL display")?;
    kept.get_or_insert_with(|| display.clone());
    Ok(display)
}

impl Headless {
    pub fn new() -> Self {
        Self {
            renderer: None,
            ipc_outputs: Default::default(),
        }
    }

    pub fn init(&mut self, _niri: &mut Niri) {}

    /// Tests that call this need a name starting with `egl_`: the Nix build has no EGL and skips
    /// them by that prefix.
    pub fn add_renderer(&mut self) -> anyhow::Result<()> {
        if self.renderer.is_some() {
            error!("add_renderer: renderer must not already exist");
            return Ok(());
        }

        let mut renderer = unsafe {
            let display = surfaceless_egl_display()?;
            let context = EGLContext::new(&display).context("error creating EGL context")?;
            GlesRenderer::new(context).context("error creating renderer")?
        };

        resources::init(&mut renderer);
        shaders::init(&mut renderer);

        self.renderer = Some(renderer);
        Ok(())
    }

    pub fn add_output(&mut self, niri: &mut Niri, n: u8, size: (u16, u16)) {
        let connector = format!("headless-{n}");
        let make = "niri".to_string();
        let model = "headless".to_string();
        let serial = n.to_string();

        let output = Output::new(
            connector.clone(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: make.clone(),
                model: model.clone(),
                serial_number: serial.clone(),
            },
        );

        let mode = Mode {
            size: Size::from((i32::from(size.0), i32::from(size.1))),
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), None, None, None);
        output.set_preferred(mode);

        output.user_data().insert_if_missing(|| OutputName {
            connector,
            make: Some(make),
            model: Some(model),
            serial: Some(serial),
        });

        let physical_properties = output.physical_properties();
        self.ipc_outputs.lock().unwrap().insert(
            OutputId::next(),
            niri_ipc::Output {
                name: output.name(),
                make: physical_properties.make,
                model: physical_properties.model,
                serial: None,
                physical_size: None,
                modes: vec![niri_ipc::Mode {
                    width: size.0,
                    height: size.1,
                    refresh_rate: 60_000,
                    is_preferred: true,
                }],
                current_mode: Some(0),
                is_custom_mode: true,
                vrr_supported: false,
                vrr_enabled: false,
                logical: Some(logical_output(&output)),
                max_bpc: None,
            },
        );

        niri.add_output(output, None, false);
    }

    pub fn seat_name(&self) -> String {
        "headless".to_owned()
    }

    pub fn with_primary_renderer<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        self.renderer.as_mut().map(f)
    }

    pub fn primary_render_node(&mut self) -> Option<DrmNode> {
        None
    }

    pub fn render(&mut self, niri: &mut Niri, output: &Output) -> RenderResult {
        let states = RenderElementStates::default();
        let mut presentation_feedbacks = niri.take_presentation_feedbacks(output, &states);
        presentation_feedbacks.presented::<_, smithay::utils::Monotonic>(
            get_monotonic_time(),
            Refresh::Unknown,
            0,
            wp_presentation_feedback::Kind::empty(),
        );

        let output_state = niri.output_state.get_mut(output).unwrap();
        match mem::replace(&mut output_state.redraw_state, RedrawState::Idle) {
            RedrawState::Idle => unreachable!(),
            RedrawState::Queued => (),
            RedrawState::WaitingForVBlank { .. } => unreachable!(),
            RedrawState::WaitingForEstimatedVBlank(_) => unreachable!(),
            RedrawState::WaitingForEstimatedVBlankAndQueued(_) => unreachable!(),
        }

        output_state.frame_callback_sequence = output_state.frame_callback_sequence.wrapping_add(1);

        // FIXME: request redraw on unfinished animations remain

        RenderResult::Submitted
    }

    pub fn import_dmabuf(&mut self, _dmabuf: &Dmabuf) -> bool {
        unimplemented!()
    }

    pub fn ipc_outputs(&self) -> Arc<Mutex<IpcOutputMap>> {
        self.ipc_outputs.clone()
    }
}

impl Default for Headless {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::egl::ffi;

    use super::*;

    // #263: dropping the last renderer must not terminate the EGL display, because another test
    // can be creating a renderer on the same display at the same time.
    #[test]
    fn egl_display_stays_initialized_after_last_renderer_is_dropped() {
        let mut headless = Headless::new();
        headless.add_renderer().unwrap();
        let renderer = headless.renderer.take().unwrap();
        let display = renderer.egl_context().display().get_display_handle().handle;
        drop(renderer);
        drop(headless);

        // eglQueryString() fails with EGL_NOT_INITIALIZED on a terminated display.
        let vendor = unsafe { ffi::egl::QueryString(display, ffi::egl::VENDOR as i32) };
        assert!(!vendor.is_null(), "EGL display was terminated");
    }
}
