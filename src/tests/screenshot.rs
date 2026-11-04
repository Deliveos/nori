use std::collections::HashMap;

use niri_config::Config;
use smithay::utils::{Physical, Rectangle, Size};
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::tests::client::ClientId;

/// GL texture ids of the screenshot UI captures of the first output, for each render target.
fn capture_texture_ids(f: &mut Fixture) -> [u32; 3] {
    let state = f.niri_state();
    let niri = &state.niri;
    state
        .backend
        .with_primary_renderer(|renderer| {
            let (_, screenshot) = niri.capture_screenshots(renderer).next().unwrap();
            screenshot.map(|s| s.texture().tex_id())
        })
        .unwrap()
}

fn set_up(config: Config) -> Fixture {
    let mut f = Fixture::with_config(config);
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (640, 480));
    f
}

#[test]
fn egl_screenshot_targets_share_texture_without_block_out() {
    let mut f = set_up(Config::default());
    let [a, b, c] = capture_texture_ids(&mut f);
    assert!(a == b && b == c, "{a} {b} {c}");
}

#[test]
fn egl_screenshot_targets_render_separately_with_block_out() {
    let config = Config::parse_mem(r#"window-rule { block-out-from "screencast"; }"#).unwrap();
    let mut f = set_up(config);
    let [a, b, c] = capture_texture_ids(&mut f);
    assert!(a != b && b != c && a != c, "{a} {b} {c}");
}

#[test]
fn screenshot_threads_are_limited() {
    use std::sync::mpsc;
    use std::time::Duration;

    use crate::niri::MAX_SCREENSHOTS_IN_FLIGHT;

    let mut f = Fixture::new();
    let niri = f.niri();

    // Threads that wait until the test lets them finish.
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
    let (done_tx, done_rx) = mpsc::channel();
    for _ in 0..MAX_SCREENSHOTS_IN_FLIGHT {
        let release_rx = release_rx.clone();
        let done_tx = done_tx.clone();
        niri.spawn_screenshot_thread(move || {
            let _ = release_rx.lock().unwrap().recv();
            let _ = done_tx.send(());
        })
        .unwrap();
    }

    // One more is refused, and so is the check before rendering one.
    assert!(niri.spawn_screenshot_thread(|| ()).is_err());
    assert!(niri.check_screenshots_in_flight().is_err());

    // Once they finish, screenshots work again.
    drop(release_tx);
    for _ in 0..MAX_SCREENSHOTS_IN_FLIGHT {
        done_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while niri.check_screenshots_in_flight().is_err() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    niri.spawn_screenshot_thread(|| ()).unwrap();
}

#[test]
fn egl_screenshot_ui_opens_without_a_screenshot_of_the_default_output() {
    let mut f = set_up(Config::default());
    f.add_output(2, (640, 480));
    let default_output = f.niri_output(1);
    let other_output = f.niri_output(2);

    let state = f.niri_state();
    let niri = &mut state.niri;
    state.backend.with_primary_renderer(|renderer| {
        // As if the default output's screenshot failed to render.
        let mut screenshots: HashMap<_, _> = niri.capture_screenshots(renderer).collect();
        screenshots.remove(&default_output);

        let ui = &mut niri.screenshot_ui;
        assert!(ui.open(renderer, screenshots, default_output, false, None));
        // Both look up the selection's output among the screenshots.
        ui.capture(renderer).unwrap();
        ui.move_left();
        assert_eq!(ui.selection_output(), Some(&other_output));
    });
}

/// Renders a window screenshot of the first window, on the first output.
fn render_window_screenshot(f: &mut Fixture) -> (Size<i32, Physical>, Vec<u8>) {
    let output = f.niri_output(1);
    let state = f.niri_state();
    let niri = &state.niri;
    let (_, mapped) = niri.layout.windows().next().unwrap();
    state
        .backend
        .with_primary_renderer(|renderer| {
            niri.render_window_screenshot(renderer, &output, mapped, false)
        })
        .unwrap()
        .unwrap()
}

/// Maps a window with an opaque `size` buffer of `rgb`, with its window geometry set to the
/// buffer, and returns its surface.
fn map_window(f: &mut Fixture, id: ClientId, size: (i32, i32), rgb: [u8; 3]) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let client = f.client(id);
    client
        .state
        .attach_shm_buffer(&surface, size.0, size.1, rgb);
    let window = client.window(&surface);
    window.xdg_surface.set_window_geometry(0, 0, size.0, size.1);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    surface
}

/// Maps a 100 × 100 green window with a 1 × 1 black subsurface at `subsurface_pos` on a
/// 640 × 480 output with `config`, and returns the size and the pixels of a window screenshot of
/// it.
fn screenshot_window_with_far_subsurface(
    config: Config,
    subsurface_pos: (i32, i32),
    set_geometry: bool,
) -> (Size<i32, Physical>, Vec<u8>) {
    let mut f = set_up(config);
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let client = f.client(id);
    client
        .state
        .attach_shm_buffer(&surface, 100, 100, [0, 255, 0]);
    client.window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);

    let (_, subsurface) = f.client(id).state.create_subsurface(&surface);
    subsurface.set_position(subsurface_pos.0, subsurface_pos.1);
    let window = f.client(id).window(&surface);
    if set_geometry {
        window.xdg_surface.set_window_geometry(0, 0, 100, 100);
    }
    window.commit();
    f.double_roundtrip(id);

    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    assert!(mapped.window.bbox_with_popups().size.w > 5000);
    // Without a window geometry, smithay uses the bounding box, which is larger than the limit.
    assert_eq!(mapped.window.geometry().size.w > 5000, !set_geometry);

    render_window_screenshot(&mut f)
}

/// The RGBA pixel at (x, y) of a `width` px wide image.
fn rgba(pixels: &[u8], width: i32, x: i32, y: i32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const TRANSPARENT: [u8; 4] = [0, 0, 0, 0];

#[test]
fn egl_window_screenshot_is_cut_around_the_window() {
    // At most the output's size plus 500 px on each side; without the cut, the image would be
    // 5001 × 5001.
    let (size, pixels) =
        screenshot_window_with_far_subsurface(Config::default(), (5000, 5000), true);
    assert_eq!(size, Size::from((1640, 1480)));
    assert_eq!(rgba(&pixels, 1640, 50, 50), GREEN);
    assert_eq!(rgba(&pixels, 1640, 1600, 1400), TRANSPARENT);
}

#[test]
fn egl_window_screenshot_is_cut_in_logical_pixels() {
    // At scale 2, the 640 × 480 output is 320 × 240 logical, so the limit is 1320 × 1240 logical,
    // or 2640 × 2480 physical.
    let config = Config::parse_mem(r#"output "headless-1" { scale 2; }"#).unwrap();
    let (size, pixels) = screenshot_window_with_far_subsurface(config, (5000, 5000), true);
    assert_eq!(size, Size::from((2640, 2480)));
    assert_eq!(rgba(&pixels, 2640, 100, 100), GREEN);
    assert_eq!(rgba(&pixels, 2640, 2600, 2400), TRANSPARENT);
}

#[test]
fn egl_window_screenshot_is_cut_without_window_geometry() {
    // Without a window geometry, the geometry reaches the far subsurface; the image keeps its
    // start.
    let (size, pixels) =
        screenshot_window_with_far_subsurface(Config::default(), (5000, 5000), false);
    assert_eq!(size, Size::from((1640, 1480)));
    assert_eq!(rgba(&pixels, 1640, 50, 50), GREEN);
}

#[test]
fn egl_window_screenshot_keeps_the_window_with_a_subsurface_before_it() {
    // The space before the window fills what the window leaves of the limit, so the window is at
    // (1540, 1380) in the image.
    let (size, pixels) =
        screenshot_window_with_far_subsurface(Config::default(), (-5000, -5000), true);
    assert_eq!(size, Size::from((1640, 1480)));
    assert_eq!(rgba(&pixels, 1640, 1590, 1430), GREEN);
    assert_eq!(rgba(&pixels, 1640, 1530, 1370), TRANSPARENT);
}

#[test]
fn egl_window_screenshot_keeps_what_is_drawn_near_a_large_window() {
    // A window almost as large as the output, with something drawn 60 px before it, such as a
    // client-drawn shadow. A cut to the output's size would leave it out.
    let mut f = set_up(Config::default());
    let id = f.add_client();
    let surface = map_window(&mut f, id, (600, 440), [0, 255, 0]);
    let (_, subsurface) = f.client(id).state.create_subsurface(&surface);
    subsurface.set_position(-60, -60);
    f.client(id).window(&surface).commit();
    f.double_roundtrip(id);

    let (size, pixels) = render_window_screenshot(&mut f);
    assert_eq!(size, Size::from((660, 500)));
    assert_eq!(rgba(&pixels, 660, 0, 0), BLACK);
    assert_eq!(rgba(&pixels, 660, 60, 60), GREEN);
}

#[test]
fn egl_window_screenshot_keeps_the_last_pixel_of_a_popup_at_a_fractional_scale() {
    // At scale 1.5, a popup at x = 101 logical is placed at 152 physical, and its 51 px end up
    // 77 px wide, so it ends at 229, while its logical end 152 × 1.5 is 228.
    let config = Config::parse_mem(r#"output "headless-1" { scale 1.5; }"#).unwrap();
    let mut f = set_up(config);
    let id = f.add_client();
    let surface = map_window(&mut f, id, (100, 100), [0, 255, 0]);

    let xdg_surface = f.client(id).window(&surface).xdg_surface.clone();
    let (popup, _, _) = f
        .client(id)
        .state
        .create_popup_at(&xdg_surface, (101, 0), (51, 10));
    f.roundtrip(id);
    f.client(id)
        .state
        .attach_shm_buffer(&popup, 51, 10, [0, 0, 255]);
    popup.commit();
    f.double_roundtrip(id);

    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    assert_eq!(
        mapped.window.bbox_with_popups(),
        Rectangle::new((0, 0).into(), (152, 100).into())
    );

    let (size, pixels) = render_window_screenshot(&mut f);
    assert_eq!(size, Size::from((229, 150)));
    assert_eq!(rgba(&pixels, 229, 228, 5), [0, 0, 255, 255]);
    assert_eq!(rgba(&pixels, 229, 149, 5), GREEN);
}

/// Maps a 100 × 100 green window with a 10 × 10 blue popup at its top-left corner, and a 1 × 1
/// subsurface of the popup at x = i32::MIN, on a 640 × 480 output with `config`, and returns the
/// size and the pixels of a window screenshot of it.
///
/// smithay saturates the bounding box with popups to x = i32::MIN, width i32::MAX, which ends at
/// x = -1, before the window geometry. Cropping it around the window overflowed (#288).
fn screenshot_window_with_popup_subsurface_at_i32_min(
    config: Config,
) -> (Size<i32, Physical>, Vec<u8>) {
    let mut f = set_up(config);
    let id = f.add_client();
    let surface = map_window(&mut f, id, (100, 100), [0, 255, 0]);

    let xdg_surface = f.client(id).window(&surface).xdg_surface.clone();
    let (popup, _, _) = f
        .client(id)
        .state
        .create_popup_at(&xdg_surface, (0, 0), (10, 10));
    f.roundtrip(id);
    f.client(id)
        .state
        .attach_shm_buffer(&popup, 10, 10, [0, 0, 255]);
    popup.commit();
    f.double_roundtrip(id);

    let (_, subsurface) = f.client(id).state.create_subsurface(&popup);
    subsurface.set_position(i32::MIN, 0);
    popup.commit();
    f.double_roundtrip(id);

    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    assert_eq!(
        mapped.window.bbox_with_popups(),
        Rectangle::new((i32::MIN, 0).into(), (i32::MAX, 100).into())
    );

    render_window_screenshot(&mut f)
}

#[test]
fn egl_popup_subsurface_at_i32_min_in_window_screenshot() {
    // The area is (-1540, 1640) logical, so it keeps the window. But the bounding box of what is
    // drawn saturates the same way, also ending at x = -1, and the image is cut to it: x from
    // -1540 to -1, none of the window.
    let (size, pixels) = screenshot_window_with_popup_subsurface_at_i32_min(Config::default());
    assert_eq!(size, Size::from((1539, 100)));
    for x in [0, 769, 1538] {
        assert_eq!(rgba(&pixels, 1539, x, 50), TRANSPARENT);
    }
}

#[test]
fn egl_popup_subsurface_at_i32_min_in_window_screenshot_below_scale_1() {
    // At scale 0.5, what is drawn is placed in physical pixels, so the subsurface is at about
    // i32::MIN / 2 and the bounding box doesn't saturate. The area is (-2180, 2280) logical,
    // (-1090, 1140) physical, and the image shows the window at x = 1090 and the popup on it.
    let config = Config::parse_mem(r#"output "headless-1" { scale 0.5; }"#).unwrap();
    let (size, pixels) = screenshot_window_with_popup_subsurface_at_i32_min(config);
    assert_eq!(size, Size::from((1140, 50)));
    assert_eq!(rgba(&pixels, 1140, 1089, 25), TRANSPARENT);
    assert_eq!(rgba(&pixels, 1140, 1092, 2), [0, 0, 255, 255]);
    assert_eq!(rgba(&pixels, 1140, 1115, 25), GREEN);
}
