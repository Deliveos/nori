use std::os::fd::OwnedFd;
use std::ptr;

use anyhow::{bail, ensure, Context as _};
use niri_config::BlockOutFrom;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer, Fourcc};
use smithay::backend::egl::fence::EGLFence;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::backend::renderer::element::{Element, Kind, RenderElement, RenderElementStates};
use smithay::backend::renderer::gles::{
    Capability, GlesError, GlesMapping, GlesRenderer, GlesTarget, GlesTexture,
};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{
    Bind, Color32F, ContextId, ExportMem, Frame, Offscreen, Renderer, Texture as _,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shm;
use solid_color::{SolidColorBuffer, SolidColorRenderElement};

use self::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use self::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::renderer::AsGlesRenderer;
use crate::render_helpers::xray::Xray;

pub mod background_effect;
pub mod blur;
pub mod border;
pub mod clipped_surface;
pub mod damage;
pub mod debug;
pub mod effect_buffer;
pub mod framebuffer_effect;
pub mod gradient_fade_texture;
pub mod memory;
pub mod offscreen;
pub mod primary_gpu_texture;
pub mod render_elements;
pub mod renderer;
pub mod resize;
pub mod resources;
pub mod shader_element;
pub mod shaders;
pub mod shadow;
pub mod snapshot;
pub mod solid_color;
pub mod surface;
pub mod texture;
pub mod xray;

/// A rendering context.
///
/// Bundles together things needed by most rendering code.
pub struct RenderCtx<'a, R> {
    pub renderer: &'a mut R,
    pub target: RenderTarget,
    pub xray: Option<&'a Xray>,
}

impl<'a, R> RenderCtx<'a, R> {
    /// Reborrows this context with a smaller lifetime.
    #[inline]
    pub fn r<'b>(&'b mut self) -> RenderCtx<'b, R> {
        RenderCtx {
            renderer: self.renderer,
            target: self.target,
            xray: self.xray,
        }
    }
}

impl<'a, R: AsGlesRenderer> RenderCtx<'a, R> {
    pub fn as_gles<'b>(&'b mut self) -> RenderCtx<'b, GlesRenderer> {
        RenderCtx {
            renderer: self.renderer.as_gles_renderer(),
            target: self.target,
            xray: self.xray,
        }
    }
}

/// What we're rendering for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTarget {
    /// Rendering to display on screen.
    Output = 0,
    /// Rendering for a screencast.
    Screencast,
    /// Rendering for any other screen capture.
    ScreenCapture,
}

/// Buffer with location, src and dst.
#[derive(Debug)]
pub struct BakedBuffer<B> {
    pub buffer: B,
    pub location: Point<f64, Logical>,
    pub src: Option<Rectangle<f64, Logical>>,
    pub dst: Option<Size<i32, Logical>>,
}

pub trait ToRenderElement {
    type RenderElement;

    fn to_render_element(
        &self,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        kind: Kind,
    ) -> Self::RenderElement;
}

impl RenderTarget {
    pub const COUNT: usize = 3;

    pub fn should_block_out(self, block_out_from: Option<BlockOutFrom>) -> bool {
        match block_out_from {
            None => false,
            Some(BlockOutFrom::Screencast) => self == RenderTarget::Screencast,
            Some(BlockOutFrom::ScreenCapture) => self != RenderTarget::Output,
        }
    }
}

impl ToRenderElement for BakedBuffer<TextureBuffer<GlesTexture>> {
    type RenderElement = PrimaryGpuTextureRenderElement;

    fn to_render_element(
        &self,
        location: Point<f64, Logical>,
        _scale: Scale<f64>,
        alpha: f32,
        kind: Kind,
    ) -> Self::RenderElement {
        let elem = TextureRenderElement::from_texture_buffer(
            self.buffer.clone(),
            location + self.location,
            alpha,
            self.src,
            self.dst.map(|dst| dst.to_f64()),
            kind,
        );
        PrimaryGpuTextureRenderElement(elem)
    }
}

impl ToRenderElement for BakedBuffer<SolidColorBuffer> {
    type RenderElement = SolidColorRenderElement;

    fn to_render_element(
        &self,
        location: Point<f64, Logical>,
        _scale: Scale<f64>,
        alpha: f32,
        kind: Kind,
    ) -> Self::RenderElement {
        SolidColorRenderElement::from_buffer(&self.buffer, location + self.location, alpha, kind)
    }
}

pub fn encompassing_geo(
    scale: Scale<f64>,
    elements: impl Iterator<Item = impl Element>,
) -> Rectangle<i32, Physical> {
    elements
        .map(|ele| ele.geometry(scale))
        .reduce(|a, b| a.merge(b))
        .unwrap_or_default()
}

/// How far past a window or a tile its contents are rendered into an offscreen texture.
///
/// Clients choose where their subsurfaces go, and with wp_viewporter how large a surface is
/// regardless of its buffer, so the area that a window's elements cover has no limit. A texture
/// sized to that area could take gigabytes of GPU memory, so animations that render a window into
/// a texture cut off what is farther than this from it.
pub const OFFSCREEN_MARGIN: f64 = 500.;

/// How large a window can be on each axis, in view sizes, for its resize animation to go through
/// textures.
///
/// The resize shader stretches each of its two textures over the whole animated area, so a texture
/// cut short of its window would leave part of that area empty. So the textures of a resize keep
/// the whole window up to this many view sizes plus [`OFFSCREEN_MARGIN`], and a larger window is
/// drawn as is while it resizes. Twice the view keeps the animation for a column twice as wide as
/// the view, and for a window that fits its output and moves to an output with the same mode at
/// twice the scale. A window that moves to an output with a much smaller logical size can be
/// larger than that, such as a maximized window moved from a 3440 × 1440 output at scale 1 to a
/// 2560 × 1600 output at scale 2 (3408 px on a 1280 px view), and is then drawn as is.
pub const RESIZE_TEXTURE_VIEWS: f64 = 2.;

/// `max_size` for [`offscreen_clip()`] of the textures of a resize animation in a view of
/// `view_size`, see [`RESIZE_TEXTURE_VIEWS`].
pub fn resize_texture_max_size(view_size: Size<f64, Logical>) -> Size<f64, Logical> {
    view_size.upscale(RESIZE_TEXTURE_VIEWS)
}

/// Area to render into an offscreen texture for a window or a tile, when its position in the view
/// isn't known.
///
/// Like [`offscreen_clip_in_view()`] with the view at the start of `extent`: on an axis where
/// `extent` is longer than `view_size`, only its start (left or top part) is kept.
pub fn offscreen_clip(
    extent: Rectangle<f64, Logical>,
    view_size: Size<f64, Logical>,
) -> Rectangle<f64, Logical> {
    offscreen_clip_in_view(extent, Rectangle::new(extent.loc, view_size))
}

/// Area to render into an offscreen texture for a window or a tile.
///
/// `extent` is what the compositor itself draws for it: the window geometry, or the tile with its
/// border, focus ring and shadow. `view` is the view it is shown in, in the same coordinates. On
/// an axis where `extent` fits into the size of `view`, all of `extent` is kept. On an axis where
/// it doesn't, only the part of `extent` as long as `view` and nearest to it is kept. Then
/// [`OFFSCREEN_MARGIN`] is added on each side.
///
/// The window size doesn't bound the area by itself: a client that doesn't set its window geometry
/// gets the bounding box of its surface and subsurfaces as the geometry, far-away subsurfaces
/// included.
pub fn offscreen_clip_in_view(
    extent: Rectangle<f64, Logical>,
    view: Rectangle<f64, Logical>,
) -> Rectangle<f64, Logical> {
    let axis = |start: f64, len: f64, view_start: f64, view_len: f64| {
        if len <= view_len {
            (start, len)
        } else {
            // Not clamp(), which panics on NaN.
            let start = f64::max(start, f64::min(view_start, start + len - view_len));
            (start, view_len)
        }
    };
    let (x, w) = axis(extent.loc.x, extent.size.w, view.loc.x, view.size.w);
    let (y, h) = axis(extent.loc.y, extent.size.h, view.loc.y, view.size.h);

    let margin = Point::from((OFFSCREEN_MARGIN, OFFSCREEN_MARGIN));
    let size = Size::from((w, h)) + margin.upscale(2.).to_size();
    Rectangle::new(Point::from((x, y)) - margin, size)
}

/// Shrinks `view` to at most `max_size` on each axis, keeping it over what it shows of `extent`.
///
/// `extent` and `view` are as in [`offscreen_clip_in_view()`], which keeps up to the size of the
/// view, so a view larger than the output would let a texture grow past the bound that it keeps
/// with a view of the output's size. A tile dragged in the overview has such a view: it is zoomed
/// out, so the output shows more of it.
///
/// On an axis where `view` is longer than `max_size`, the result is as long as `max_size` and
/// centered on the part of `extent` inside `view`, or on `view` when no part of `extent` is inside
/// it. Then the clip keeps all of that part when it is at most `max_size` plus
/// [`OFFSCREEN_MARGIN`] on each side.
pub fn limit_view_size(
    extent: Rectangle<f64, Logical>,
    view: Rectangle<f64, Logical>,
    max_size: Size<f64, Logical>,
) -> Rectangle<f64, Logical> {
    let axis = |start: f64, len: f64, view_start: f64, view_len: f64, max_len: f64| {
        if view_len <= max_len {
            return (view_start, view_len);
        }

        let shown_start = f64::max(start, view_start);
        let shown_end = f64::min(start + len, view_start + view_len);
        let center = if shown_start <= shown_end {
            (shown_start + shown_end) / 2.
        } else {
            view_start + view_len / 2.
        };
        (center - max_len / 2., max_len)
    };
    let (x, w) = axis(
        extent.loc.x,
        extent.size.w,
        view.loc.x,
        view.size.w,
        max_size.w,
    );
    let (y, h) = axis(
        extent.loc.y,
        extent.size.h,
        view.loc.y,
        view.size.h,
        max_size.h,
    );
    Rectangle::new(Point::from((x, y)), Size::from((w, h)))
}

/// Area that the parts of `elements` inside `clip` cover.
pub fn clipped_encompassing_geo(
    scale: Scale<f64>,
    elements: impl Iterator<Item = impl Element>,
    clip: Rectangle<f64, Logical>,
) -> Rectangle<i32, Physical> {
    let clip = clip.to_physical_precise_up(scale);
    elements
        .filter_map(|ele| ele.geometry(scale).intersection(clip))
        .reduce(|a, b| a.merge(b))
        .unwrap_or_default()
}

pub fn create_texture(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
    fourcc: Fourcc,
) -> Result<GlesTexture, GlesError> {
    let buffer_size = size.to_logical(1).to_buffer(1, Transform::Normal);
    renderer.create_buffer(fourcc, buffer_size)
}

/// Offscreen texture kept across the frames of a capture, so that each frame doesn't allocate a
/// new full-size one.
#[derive(Debug, Default)]
pub struct CaptureTexture(Option<(ContextId<GlesTexture>, Fourcc, GlesTexture)>);

impl CaptureTexture {
    /// Returns the texture, creating it if the size, the format or the renderer changed.
    pub(crate) fn get(
        &mut self,
        renderer: &mut GlesRenderer,
        size: Size<i32, Physical>,
        fourcc: Fourcc,
    ) -> Result<&mut GlesTexture, GlesError> {
        let context_id = renderer.context_id();
        let buffer_size = size.to_logical(1).to_buffer(1, Transform::Normal);
        let reusable = self.0.as_ref().is_some_and(|(id, format, texture)| {
            *id == context_id && *format == fourcc && texture.size() == buffer_size
        });
        if !reusable {
            // Free the old one before allocating, there may not be room for both.
            self.0 = None;
        }
        let (_, _, texture) = match &mut self.0 {
            Some(entry) => entry,
            slot @ None => {
                let texture = create_texture(renderer, size, fourcc)?;
                slot.insert((context_id, fourcc, texture))
            }
        };
        Ok(texture)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Frees the texture; the next capture allocates a new one.
    pub fn clear(&mut self) {
        self.0 = None;
    }
}

pub fn copy_framebuffer(
    renderer: &mut GlesRenderer,
    target: &GlesTarget,
    fourcc: Fourcc,
) -> Result<GlesMapping, GlesError> {
    renderer.copy_framebuffer(target, Rectangle::from_size(target.size()), fourcc)
}

/// Renders `elements` into a texture covering them, or only their part inside `clip`.
pub fn render_to_encompassing_texture(
    renderer: &mut GlesRenderer,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
    elements: &[impl RenderElement<GlesRenderer>],
    clip: Option<Rectangle<f64, Logical>>,
) -> anyhow::Result<(GlesTexture, SyncPoint, Rectangle<i32, Physical>)> {
    let mut geo = match clip {
        Some(clip) => clipped_encompassing_geo(scale, elements.iter(), clip),
        None => encompassing_geo(scale, elements.iter()),
    };
    // Guard against nothing inside the clip (or no elements) producing a zero size, which can't be
    // bound, like OffscreenBuffer does.
    if geo.size.w == 0 || geo.size.h == 0 {
        geo.size = Size::new(1, 1);
    }
    let elements = elements.iter().rev().map(|ele| {
        RelocateRenderElement::from_element(ele, geo.loc.upscale(-1), Relocate::Relative)
    });

    let (texture, sync_point) =
        render_to_texture(renderer, geo.size, scale, transform, fourcc, elements)?;

    Ok((texture, sync_point, geo))
}

pub fn render_to_texture(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
    elements: impl Iterator<Item = impl RenderElement<GlesRenderer>>,
) -> anyhow::Result<(GlesTexture, SyncPoint)> {
    let _span = tracy_client::span!();

    let mut texture = create_texture(renderer, size, fourcc).context("error creating texture")?;

    let sync_point = {
        let mut target = renderer
            .bind(&mut texture)
            .context("error binding texture")?;

        render_elements(renderer, &mut target, size, scale, transform, elements)?
    };

    Ok((texture, sync_point))
}

pub fn render_and_download(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
    elements: impl Iterator<Item = impl RenderElement<GlesRenderer>>,
) -> anyhow::Result<GlesMapping> {
    let _span = tracy_client::span!();

    let mut texture = create_texture(renderer, size, fourcc).context("error creating texture")?;
    let mut target = renderer
        .bind(&mut texture)
        .context("error binding texture")?;

    let _sync = render_elements(renderer, &mut target, size, scale, transform, elements)
        .context("error rendering")?;

    copy_framebuffer(renderer, &target, fourcc).context("error copying framebuffer")
}

pub fn render_and_download_with_damage(
    renderer: &mut GlesRenderer,
    damage_tracker: &mut OutputDamageTracker,
    texture: &mut CaptureTexture,
    fourcc: Fourcc,
    elements: &[impl RenderElement<GlesRenderer>],
    states: RenderElementStates,
) -> anyhow::Result<PendingDownload> {
    let _span = tracy_client::span!();

    let (size, _scale, _transform) = damage_tracker.mode().try_into().unwrap();
    let texture = texture
        .get(renderer, size, fourcc)
        .context("error creating texture")?;
    let mut target = renderer.bind(texture).context("error binding texture")?;

    let _res = damage_tracker
        .render_output_with_states(
            renderer,
            &mut target,
            0,
            elements,
            Color32F::TRANSPARENT,
            states,
        )
        .context("error rendering")?;

    start_download(renderer, &target, fourcc)
}

/// Pixels on their way back from the GPU.
#[derive(Debug)]
pub struct PendingDownload {
    mapping: GlesMapping,
    context_id: ContextId<GlesTexture>,
    /// Reached once the GPU finished the readback, if fences are supported.
    sync: Option<SyncPoint>,
}

impl PendingDownload {
    /// Returns an fd that becomes readable when [`Self::map()`] no longer waits for the GPU.
    ///
    /// `None` without fence support; then `map()` waits for the GPU.
    pub fn sync_fd(&self) -> Option<OwnedFd> {
        self.sync.as_ref().and_then(SyncPoint::export)
    }

    /// Returns the sync point reached when the readback is done, signaled without fences.
    pub fn sync_point(&self) -> SyncPoint {
        self.sync.clone().unwrap_or_else(SyncPoint::signaled)
    }

    pub fn map<'a>(&'a self, renderer: &mut GlesRenderer) -> anyhow::Result<&'a [u8]> {
        // The mapping belongs to the renderer that rendered the frame.
        ensure!(
            renderer.context_id() == self.context_id,
            "renderer changed during the download"
        );
        renderer
            .map_texture(&self.mapping)
            .context("error mapping texture")
    }
}

/// Starts reading the target back from the GPU, without waiting for it.
fn start_download(
    renderer: &mut GlesRenderer,
    target: &GlesTarget,
    fourcc: Fourcc,
) -> anyhow::Result<PendingDownload> {
    let mapping =
        copy_framebuffer(renderer, target, fourcc).context("error copying framebuffer")?;
    let context_id = renderer.context_id();

    // A fence after the readback tells when mapping it stops blocking.
    let mut sync = None;
    if renderer.capabilities().contains(&Capability::ExportFence) {
        let display = renderer.egl_context().display().clone();
        let fence = renderer.with_context(|gl| {
            let fence = EGLFence::create(&display);
            // The fence only gets an fd once it's flushed.
            unsafe { gl.Flush() };
            fence
        });
        match fence {
            Ok(Ok(fence)) => sync = Some(SyncPoint::from(fence)),
            Ok(Err(err)) => debug!("error creating fence for download: {err:?}"),
            Err(err) => debug!("error flushing for download: {err:?}"),
        }
    }

    Ok(PendingDownload {
        mapping,
        context_id,
        sync,
    })
}

pub fn render_to_vec(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    fourcc: Fourcc,
    elements: impl Iterator<Item = impl RenderElement<GlesRenderer>>,
) -> anyhow::Result<Vec<u8>> {
    let _span = tracy_client::span!();

    let mapping = render_and_download(renderer, size, scale, transform, fourcc, elements)
        .context("error rendering")?;
    let copy = renderer
        .map_texture(&mapping)
        .context("error mapping texture")?;
    Ok(copy.to_vec())
}

/// Renders into the DMA-BUF. `age` is the buffer age for the damage tracker, 0 if unknown.
pub fn render_to_dmabuf(
    renderer: &mut GlesRenderer,
    damage_tracker: &mut OutputDamageTracker,
    mut dmabuf: Dmabuf,
    age: usize,
    elements: &[impl RenderElement<GlesRenderer>],
    states: RenderElementStates,
) -> anyhow::Result<SyncPoint> {
    let _span = tracy_client::span!();
    let (size, _scale, _transform) = damage_tracker.mode().try_into().unwrap();
    ensure!(
        dmabuf.width() == size.w as u32 && dmabuf.height() == size.h as u32,
        "invalid buffer size"
    );

    let mut target = renderer.bind(&mut dmabuf).context("error binding dmabuf")?;
    let res = damage_tracker
        .render_output_with_states(
            renderer,
            &mut target,
            age,
            elements,
            Color32F::TRANSPARENT,
            states,
        )
        .context("error rendering to dmabuf")?;
    Ok(res.sync)
}

/// A frame rendered for a capture client's buffer, but not in it yet.
pub enum PendingCapture {
    /// In the client's DMA-BUF once the sync point is reached.
    Dma(SyncPoint),
    /// To be copied into the client's SHM buffer once the GPU is done.
    Shm(PendingShmCopy),
}

/// A frame rendered for an SHM buffer, waiting to be copied into it.
#[derive(Debug)]
pub struct PendingShmCopy {
    download: PendingDownload,
    buffer: WlBuffer,
    format: wl_shm::Format,
    size: Size<i32, Physical>,
}

/// Renders a frame for the SHM `buffer` and starts reading it back from the GPU.
///
/// [`PendingShmCopy::finish()`] then copies it into the buffer.
pub fn render_for_shm(
    renderer: &mut GlesRenderer,
    damage_tracker: &mut OutputDamageTracker,
    texture: &mut CaptureTexture,
    buffer: &WlBuffer,
    format: wl_shm::Format,
    elements: &[impl RenderElement<GlesRenderer>],
    states: RenderElementStates,
) -> anyhow::Result<PendingShmCopy> {
    let _span = tracy_client::span!();

    let (size, _scale, _transform) = damage_tracker.mode().try_into().unwrap();
    let fourcc = match format {
        wl_shm::Format::Xrgb8888 => Fourcc::Xrgb8888,
        wl_shm::Format::Argb8888 => Fourcc::Argb8888,
        _ => bail!("unsupported shm format: {format:?}"),
    };
    shm::with_buffer_contents(buffer, |_, pool_len, buffer_data| {
        check_shm_buffer(buffer_data, pool_len, format, size)
    })
    .context("expected shm buffer, but didn't get one")??;

    let texture = texture
        .get(renderer, size, fourcc)
        .context("error creating texture")?;
    let mut target = renderer.bind(texture).context("error binding texture")?;

    let _res = damage_tracker
        .render_output_with_states(
            renderer,
            &mut target,
            0,
            elements,
            Color32F::TRANSPARENT,
            states,
        )
        .context("error rendering")?;

    Ok(PendingShmCopy {
        download: start_download(renderer, &target, fourcc)?,
        buffer: buffer.clone(),
        format,
        size,
    })
}

impl PendingShmCopy {
    /// See [`PendingDownload::sync_fd()`].
    pub fn sync_fd(&self) -> Option<OwnedFd> {
        self.download.sync_fd()
    }

    /// Copies the frame into the buffer, waiting for the GPU if it isn't done yet.
    pub fn finish(self, renderer: &mut GlesRenderer) -> anyhow::Result<()> {
        let _span = tracy_client::span!("PendingShmCopy::finish");

        let bytes = self.download.map(renderer)?;
        let size = self.size;

        // The pointer and length are the client's entire pool, which may hold other
        // buffers besides this one... buffer_data is the one we want.
        shm::with_buffer_contents_mut(&self.buffer, |pool, pool_len, buffer_data| {
            let (offset, stride) = check_shm_buffer(buffer_data, pool_len, self.format, size)?;

            let row_len = size.w as usize * 4;
            let height = size.h as usize;
            ensure!(bytes.len() >= row_len * height, "short texture mapping");

            unsafe {
                let _span = tracy_client::span!("copy_nonoverlapping");
                let dst = pool.add(offset);
                if stride == row_len {
                    ptr::copy_nonoverlapping(bytes.as_ptr(), dst, row_len * height);
                } else {
                    for y in 0..height {
                        ptr::copy_nonoverlapping(
                            bytes.as_ptr().add(y * row_len),
                            dst.add(y * stride),
                            row_len,
                        );
                    }
                }
            }

            Ok(())
        })
        .context("expected shm buffer, but didn't get one")?
    }
}

/// Checks that the SHM buffer fits a frame of `size` in `format`, and returns its offset and
/// stride.
fn check_shm_buffer(
    buffer_data: shm::BufferData,
    pool_len: usize,
    format: wl_shm::Format,
    size: Size<i32, Physical>,
) -> anyhow::Result<(usize, usize)> {
    ensure!(
        // The buffer prefers pixels in little endian ...
        buffer_data.format == format && buffer_data.width == size.w && buffer_data.height == size.h,
        "invalid buffer format or size"
    );

    // The client chooses the stride and may pad rows, so only the first
    // row_len bytes can be used here.
    let row_len = size.w as usize * 4;
    let height = size.h as usize;
    let offset = usize::try_from(buffer_data.offset).context("negative buffer offset")?;
    let stride = usize::try_from(buffer_data.stride).context("negative buffer stride")?;

    // This should have already been validated by wl_shm, and a pool can
    // only grow, but check again just in case.
    let end = stride
        .checked_mul(height.saturating_sub(1))
        .and_then(|len| len.checked_add(row_len))
        .and_then(|len| len.checked_add(offset));
    ensure!(
        stride >= row_len && end.is_some_and(|end| end <= pool_len),
        "buffer does not fit in its shm pool"
    );

    Ok((offset, stride))
}

pub fn clear_dmabuf(renderer: &mut GlesRenderer, mut dmabuf: Dmabuf) -> anyhow::Result<SyncPoint> {
    let size = dmabuf.size();
    let size = size.to_logical(1, Transform::Normal).to_physical(1);
    let mut target = renderer.bind(&mut dmabuf).context("error binding dmabuf")?;
    let mut frame = renderer
        .render(&mut target, size, Transform::Normal)
        .context("error starting frame")?;
    frame
        .clear(Color32F::TRANSPARENT, &[Rectangle::from_size(size)])
        .context("error clearing")?;
    frame.finish().context("error finishing frame")
}

fn render_elements(
    renderer: &mut GlesRenderer,
    target: &mut GlesTarget,
    size: Size<i32, Physical>,
    scale: Scale<f64>,
    transform: Transform,
    elements: impl Iterator<Item = impl RenderElement<GlesRenderer>>,
) -> anyhow::Result<SyncPoint> {
    let transform = transform.invert();
    let output_rect = Rectangle::from_size(transform.transform_size(size));

    let mut frame = renderer
        .render(target, size, transform)
        .context("error starting frame")?;

    frame
        .clear(Color32F::TRANSPARENT, &[output_rect])
        .context("error clearing")?;

    for element in elements {
        let src = element.src();
        let dst = element.geometry(scale);

        if let Some(mut damage) = output_rect.intersection(dst) {
            damage.loc -= dst.loc;

            let cache = UserDataMap::new();
            if element.is_framebuffer_effect() {
                element
                    .capture_framebuffer(&mut frame, src, dst, &cache)
                    .context("error in capture_framebuffer()")?;
            }
            element
                .draw(&mut frame, src, dst, &[damage], &[], Some(&cache))
                .context("error drawing element")?;
        }
    }

    frame.finish().context("error finishing frame")
}

#[cfg(test)]
mod tests {
    use smithay::backend::egl::native::EGLSurfacelessDisplay;
    use smithay::backend::egl::{EGLContext, EGLDisplay};

    use super::*;

    fn renderer() -> GlesRenderer {
        unsafe {
            let display = EGLDisplay::new(EGLSurfacelessDisplay).unwrap();
            let context = EGLContext::new(&display).unwrap();
            GlesRenderer::new(context).unwrap()
        }
    }

    #[test]
    fn egl_capture_texture_is_reused_until_size_or_format_changes() {
        let mut renderer = renderer();

        let mut texture = CaptureTexture::default();
        let size = Size::from((64, 32));
        let first = texture.get(&mut renderer, size, Fourcc::Xrgb8888).unwrap();
        let first = first.tex_id();
        let again = texture.get(&mut renderer, size, Fourcc::Xrgb8888).unwrap();
        assert_eq!(again.tex_id(), first);

        let size = Size::from((32, 32));
        let resized = texture.get(&mut renderer, size, Fourcc::Xrgb8888).unwrap();
        assert_eq!(resized.size(), Size::from((32, 32)));
        let reformatted = texture.get(&mut renderer, size, Fourcc::Argb8888).unwrap();
        assert_eq!(reformatted.format(), Some(Fourcc::Argb8888));
    }

    #[test]
    fn egl_download_has_the_rendered_pixels() {
        let mut renderer = renderer();
        let mut damage_tracker = OutputDamageTracker::new((4, 2), 1., Transform::Normal);
        let mut texture = CaptureTexture::default();

        // The second frame renders into the same texture.
        for (color, expected) in [
            (Color32F::new(1., 0., 0., 1.), [255, 0, 0, 255]),
            (Color32F::new(0., 1., 0., 1.), [0, 255, 0, 255]),
        ] {
            let buffer = SolidColorBuffer::new((4., 2.), color);
            let elements = [SolidColorRenderElement::from_buffer(
                &buffer,
                (0., 0.),
                1.,
                Kind::Unspecified,
            )];
            let download = render_and_download_with_damage(
                &mut renderer,
                &mut damage_tracker,
                &mut texture,
                Fourcc::Abgr8888,
                &elements,
                RenderElementStates::default(),
            )
            .unwrap();

            // Abgr8888 is R, G, B, A in memory.
            let bytes = download.map(&mut renderer).unwrap();
            assert_eq!(bytes.len(), 4 * 2 * 4);
            assert!(bytes.chunks(4).all(|px| px == expected), "{bytes:?}");
        }
    }

    fn solid(buffer: &SolidColorBuffer, loc: (f64, f64)) -> SolidColorRenderElement {
        SolidColorRenderElement::from_buffer(buffer, loc, 1., Kind::Unspecified)
    }

    #[test]
    fn offscreen_clip_adds_the_margin_up_to_the_view_size() {
        let view = Size::from((1920., 1080.));
        let rect = |x, y, w, h| Rectangle::new(Point::from((x, y)), Size::from((w, h)));

        // A window that fits gets the margin on each side.
        let window = rect(0., 0., 100., 50.);
        assert_eq!(
            offscreen_clip(window, view),
            rect(-500., -500., 1100., 1050.)
        );

        // A tile with its shadow starts before the origin.
        let tile = rect(-40., -30., 180., 130.);
        assert_eq!(offscreen_clip(tile, view), rect(-540., -530., 1180., 1130.));

        // A window larger than the view on an axis, as with a far-away subsurface and no window
        // geometry set, keeps only its start on that axis.
        let huge = rect(0., 0., 16001., 50.);
        assert_eq!(offscreen_clip(huge, view), rect(-500., -500., 2920., 1050.));
    }

    #[test]
    fn offscreen_clip_in_view_keeps_the_part_near_the_view() {
        let rect = |x, y, w, h| Rectangle::new(Point::from((x, y)), Size::from((w, h)));
        let window = rect(0., 0., 5000., 50.);

        // A view showing the middle of a window wider than it.
        let view = rect(3000., -200., 1920., 1080.);
        let clip = offscreen_clip_in_view(window, view);
        assert_eq!(clip, rect(2500., -500., 2920., 1050.));

        // A view overlapping the end of the window keeps the end.
        let view = rect(4000., 0., 1920., 1080.);
        let clip = offscreen_clip_in_view(window, view);
        assert_eq!(clip, rect(2580., -500., 2920., 1050.));

        // A view before the start of the window keeps the start.
        let view = rect(-3000., 0., 1920., 1080.);
        let clip = offscreen_clip_in_view(window, view);
        assert_eq!(clip, rect(-500., -500., 2920., 1050.));

        // A window that fits into the view is kept whole wherever the view is.
        let view = rect(-3000., 9000., 1920., 1080.);
        let clip = offscreen_clip_in_view(rect(10., 20., 100., 50.), view);
        assert_eq!(clip, rect(-490., -480., 1100., 1050.));
    }

    #[test]
    fn limit_view_size_keeps_what_a_zoomed_out_view_shows() {
        let rect = |x, y, w, h| Rectangle::new(Point::from((x, y)), Size::from((w, h)));
        let max_size = Size::from((1920., 1080.));

        // A view no larger than the maximum stays as it is.
        let view = rect(100., -50., 1920., 1080.);
        assert_eq!(
            limit_view_size(rect(0., 0., 50., 50.), view, max_size),
            view
        );

        // A 200 % column (3792 × 992 px) with a 4 px focus ring, dragged in the overview at zoom
        // 0.5 so that it is drawn from x = -600 on a 1920 × 1080 output: the output shows tile x
        // 1200 to 5040, and y -200 to 1960. The tile reaches to x = 3796, so 2596 px of it are
        // shown on that axis, and the clip keeps them all.
        let tile = rect(-4., -4., 3800., 1000.);
        let view = rect(1200., -200., 3840., 2160.);
        let limited = limit_view_size(tile, view, max_size);
        assert_eq!(limited, rect(1538., -44., 1920., 1080.));
        let clip = offscreen_clip_in_view(tile, limited);
        assert_eq!(clip, rect(1038., -504., 2920., 2000.));
        assert!(clip.contains_rect(tile.intersection(view).unwrap()));

        // A window much larger than the view, at zoom 0.05: the clip is no larger than with an
        // unzoomed view.
        let window = rect(0., 0., 15000., 15000.);
        let view = rect(-1000., 2000., 38400., 21600.);
        let clip = offscreen_clip_in_view(window, limit_view_size(window, view, max_size));
        assert_eq!(clip, rect(6040., 7460., 2920., 2080.));

        // On an axis where the view shows nothing of the window, it is limited around its own
        // center.
        let view = rect(20000., 0., 3840., 2160.);
        assert_eq!(
            limit_view_size(window, view, max_size),
            rect(20960., 540., 1920., 1080.)
        );
    }

    #[test]
    fn clipped_encompassing_geo_leaves_out_far_elements() {
        let scale = Scale::from(2.);
        let clip = offscreen_clip(
            Rectangle::from_size((10., 10.).into()),
            (1920., 1080.).into(),
        );
        let geo = |elements: &[SolidColorRenderElement]| {
            clipped_encompassing_geo(scale, elements.iter(), clip)
        };

        let window = SolidColorBuffer::new((10., 10.), [0., 0., 0., 1.]);
        let tiny = SolidColorBuffer::new((1., 1.), [0., 0., 0., 1.]);
        let wide = SolidColorBuffer::new((4000., 1.), [0., 0., 0., 1.]);

        // A tiny subsurface far away doesn't make the area larger.
        let far = [solid(&window, (0., 0.)), solid(&tiny, (16000., 16000.))];
        assert_eq!(geo(&far), Rectangle::from_size((20, 20).into()));

        // One within the margin does.
        let near = [solid(&window, (0., 0.)), solid(&tiny, (-100., 200.))];
        assert_eq!(
            geo(&near),
            Rectangle::new((-200, 0).into(), (220, 402).into())
        );

        // A huge surface is cut at the margin, 500 logical px past the window.
        let huge = [solid(&window, (0., 0.)), solid(&wide, (-2000., 0.))];
        assert_eq!(
            geo(&huge),
            Rectangle::new((-1000, 0).into(), (2020, 20).into())
        );

        let nothing_inside = [solid(&tiny, (16000., 16000.))];
        assert_eq!(geo(&nothing_inside), Rectangle::default());
    }

    #[test]
    fn egl_encompassing_texture_is_cut_to_the_clip() {
        let mut renderer = renderer();
        let scale = Scale::from(1.);

        let window = SolidColorBuffer::new((10., 10.), Color32F::new(0., 1., 0., 1.));
        let wide = SolidColorBuffer::new((4000., 1.), Color32F::new(1., 0., 0., 1.));
        let tiny = SolidColorBuffer::new((1., 1.), Color32F::new(0., 0., 1., 1.));
        // Elements go from top to bottom.
        let elements = [
            solid(&window, (0., 0.)),
            solid(&wide, (-2000., 0.)),
            solid(&tiny, (3000., 3000.)),
        ];

        let clip = offscreen_clip(
            Rectangle::from_size((10., 10.).into()),
            (1920., 1080.).into(),
        );
        let (mut texture, _sync, geo) = render_to_encompassing_texture(
            &mut renderer,
            scale,
            Transform::Normal,
            Fourcc::Abgr8888,
            &elements,
            Some(clip),
        )
        .unwrap();
        assert_eq!(geo, Rectangle::new((-500, 0).into(), (1010, 10).into()));
        assert_eq!(texture.size(), Size::from((1010, 10)));

        let target = renderer.bind(&mut texture).unwrap();
        let mapping = copy_framebuffer(&mut renderer, &target, Fourcc::Abgr8888).unwrap();
        drop(target);
        let bytes = renderer.map_texture(&mapping).unwrap();

        // Abgr8888 is R, G, B, A in memory. The wide surface is in one row, under the window.
        let count = |px: [u8; 4]| bytes.chunks(4).filter(|p| *p == px).count();
        assert_eq!(count([0, 255, 0, 255]), 10 * 10);
        assert_eq!(count([255, 0, 0, 255]), 1010 - 10);
        assert_eq!(count([0, 0, 0, 0]), 1010 * 10 - 10 * 10 - 1000);
    }
}
