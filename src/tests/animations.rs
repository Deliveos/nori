use std::fmt::Write as _;
use std::iter::zip;
use std::time::Duration;

use insta::assert_snapshot;
use niri_config::animations::{Curve, EasingParams, Kind};
use niri_config::Config;
use niri_ipc::{PositionChange, SizeChange};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::{Bind as _, Color32F, ExportMem as _};
use smithay::utils::{Point, Scale, Size};
use wayland_client::protocol::wl_surface::WlSurface;

use super::client::ClientId;
use super::*;
use crate::layout::ActivateWindow;
use crate::niri::{Niri, State};
use crate::render_helpers::opaque_limit::limit_opaque_regions;
use crate::render_helpers::{copy_framebuffer, create_texture, RenderCtx, RenderTarget};

fn format_tiles(niri: &Niri) -> String {
    let mut buf = String::new();
    let ws = niri.layout.active_workspace().unwrap();
    let mut tiles: Vec<_> = ws.tiles_with_render_positions().collect();

    // We sort by id since that gives us a consistent order (from first opened to last), but we
    // don't print the id since it's nondeterministic (the id is a global counter across all
    // running tests in the same binary).
    tiles.sort_by_key(|(tile, _, _)| tile.window().id().get());
    for (tile, pos, _visible) in tiles {
        let Size { w, h, .. } = tile.animated_tile_size();
        let Point { x, y, .. } = pos;
        writeln!(&mut buf, "{w:>3.0} × {h:>3.0} at x:{x:>3.0} y:{y:>3.0}").unwrap();
    }
    buf
}

fn create_window(f: &mut Fixture, id: ClientId, w: u16, h: u16) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(w, h);
    window.ack_last_and_commit();
    f.roundtrip(id);

    surface
}

fn set_time(niri: &mut Niri, time: Duration) {
    // This is a bit involved because we're dealing with an AdjustableClock that maintains its own
    // internal current_time.

    // First, reset current_time to zero by matching unadjusted time to it (at rate 0.0), then
    // setting unadjusted time to zero at rate 1.0 (causing current_time to also go to zero).
    let now = niri.clock.now();
    niri.clock.set_unadjusted(now);
    let _ = niri.clock.now();
    niri.clock.set_unadjusted(Duration::ZERO);
    niri.clock.set_rate(1.0);
    let _ = niri.clock.now();

    // Now, set the desired time at rate 1.0.
    niri.clock.set_unadjusted(time);
    let _ = niri.clock.now();

    // Freeze the clock so that clear() inside the niri loop callback followed by some get()
    // doesn't replace it with the monotonic time.
    niri.clock.set_rate(0.0);
}

// Sets up a fixture with linear animations, a renderer, and an output.
fn set_up() -> Fixture {
    set_up_with(|_| ())
}

fn set_up_with(adjust_config: impl FnOnce(&mut Config)) -> Fixture {
    const LINEAR: Kind = Kind::Easing(EasingParams {
        duration_ms: 1000,
        curve: Curve::Linear,
    });

    let mut config = Config::default();
    config.layout.gaps = 0.0;
    config.animations.window_resize.anim.kind = LINEAR;
    config.animations.window_movement.0.kind = LINEAR;
    // Keep the overlay out of the rendered pixels.
    config.hotkey_overlay.skip_at_startup = true;
    adjust_config(&mut config);

    let mut f = Fixture::with_config(config);
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (1920, 1080));

    f
}

// Renders an output with a 1920×1080 mode the way a backend does, including the visibility
// tracking, and returns the pixels as RGBA.
fn render_output(f: &mut Fixture, n: u8) -> Vec<u8> {
    let output = f.niri_output(n);
    let State { backend, niri, .. } = f.niri_state();
    niri.update_render_elements(Some(&output));

    backend
        .with_primary_renderer(|renderer| {
            let ctx = RenderCtx {
                renderer: &mut *renderer,
                target: RenderTarget::Output,
                xray: None,
            };
            let elements = niri.render_to_vec(ctx, &output, false);
            let scale = Scale::from(output.current_scale().fractional_scale());
            let elements = limit_opaque_regions(&elements, scale);

            let mut damage_tracker = OutputDamageTracker::from_output(&output);
            let size = Size::from((1920, 1080));
            let mut texture = create_texture(renderer, size, Fourcc::Abgr8888).unwrap();
            let mut target = renderer.bind(&mut texture).unwrap();
            let res = damage_tracker
                .render_output(renderer, &mut target, 0, &elements, Color32F::TRANSPARENT)
                .unwrap();
            niri.update_primary_scanout_output(&output, &res.states);

            let mapping = copy_framebuffer(renderer, &target, Fourcc::Abgr8888).unwrap();
            renderer.map_texture(&mapping).unwrap().to_vec()
        })
        .unwrap()
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 3] {
    let i = (y * 1920 + x) * 4;
    [pixels[i], pixels[i + 1], pixels[i + 2]]
}

fn set_up_two_in_column() -> (Fixture, ClientId, WlSurface, WlSurface) {
    let mut f = set_up();

    let id = f.add_client();

    let surface1 = create_window(&mut f, id, 100, 100);
    let surface2 = create_window(&mut f, id, 200, 200);
    f.double_roundtrip(id);

    let _ = f.client(id).window(&surface1).recent_configures();
    let _ = f.client(id).window(&surface2).recent_configures();

    // Consume into one column.
    f.niri().layout.focus_left();
    f.niri().layout.consume_into_column();
    f.double_roundtrip(id);

    // Commit for the column consume.
    let window = f.client(id).window(&surface1);
    window.ack_last_and_commit();

    let window = f.client(id).window(&surface2);
    window.ack_last_and_commit();

    f.double_roundtrip(id);

    set_time(f.niri(), Duration::ZERO);
    f.niri_complete_animations();

    (f, id, surface1, surface2)
}

#[test]
fn egl_height_resize_animates_next_y() {
    let (mut f, id, surface1, surface2) = set_up_two_in_column();

    // Issue a resize.
    f.niri()
        .layout
        .set_window_height(None, SizeChange::AdjustFixed(-50));
    f.double_roundtrip(id);

    // The top window shrinks in response, the bottom remains as is.
    let window = f.client(id).window(&surface1);
    window.set_size(100, 50);
    window.ack_last_and_commit();
    let window = f.client(id).window(&surface2);
    window.ack_last_and_commit();

    // This starts the resize animation for the top window and the Y move for the bottom.
    f.roundtrip(id);

    // No time had passed yet, so we're at the initial state.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    // Advance the time halfway.
    set_time(f.niri(), Duration::from_millis(500));
    f.niri().advance_animations();

    // Top window is half-resized at 75 px tall, bottom window is at y=75 matching it.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 ×  75 at x:  0 y:  0
    200 × 200 at x:  0 y: 75
    ");

    // Advance the time to completion.
    set_time(f.niri(), Duration::from_millis(1000));
    f.niri().advance_animations();

    // Final state at 50 px.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 ×  50 at x:  0 y:  0
    200 × 200 at x:  0 y: 50
    ");
}

#[test]
fn egl_clientside_height_change_doesnt_animate() {
    let (mut f, id, surface1, _surface2) = set_up_two_in_column();

    // The initial state.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 × 100 at x:  0 y:  0
    200 × 200 at x:  0 y:100
    ");

    // The top window shrinks by itself, without a niri-issued resize.
    let window = f.client(id).window(&surface1);
    window.set_size(100, 50);
    window.commit();

    // This does not start any animations.
    f.roundtrip(id);

    // No time had passed yet, but we are at the final state right away.
    assert_snapshot!(format_tiles(f.niri()), @r"
    100 ×  50 at x:  0 y:  0
    200 × 200 at x:  0 y: 50
    ");
}

#[test]
fn egl_resize_animation_starts_from_what_was_on_screen() {
    const FRAME: [u8; 3] = [0, 0, 255];
    const CONTENT: [u8; 3] = [0, 255, 0];

    let mut f = set_up();
    let id = f.add_client();

    // Like Firefox, the window draws its contents on a desynchronized subsurface.
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let (content, subsurface) = f.client(id).state.create_subsurface(&surface);
    subsurface.set_desync();

    let resize = |f: &mut Fixture, w: i32| {
        let client = f.client(id);
        // The contents change size first, and only then the window acks the configure.
        client.state.attach_shm_buffer(&content, w, 1080, CONTENT);
        content.commit();
        client.state.attach_shm_buffer(&surface, w, 1080, FRAME);
        let window = client.window(&surface);
        window.set_size(w as u16, 1080);
        window.ack_last_and_commit();
        f.roundtrip(id);
    };

    resize(&mut f, 960);
    f.double_roundtrip(id);
    set_time(f.niri(), Duration::ZERO);
    f.niri_complete_animations();
    render_output(&mut f, 1);

    f.niri().layout.toggle_full_width();
    f.double_roundtrip(id);
    resize(&mut f, 1920);
    f.niri_complete_animations();
    let pixels = render_output(&mut f, 1);
    assert_eq!(pixel(&pixels, 1440, 540), CONTENT);

    // Back to half width. At the start of the animation, the window should look the way it did
    // before the resize, not like the full-size frame with the half-size contents.
    f.niri().layout.toggle_full_width();
    f.double_roundtrip(id);
    resize(&mut f, 960);
    let pixels = render_output(&mut f, 1);
    assert_eq!(pixel(&pixels, 480, 540), CONTENT);
    assert_eq!(pixel(&pixels, 1440, 540), CONTENT);
}

#[test]
fn egl_resize_animation_off_takes_no_configure_snapshot() {
    for (window_resize_off, all_off) in [(false, false), (true, false), (false, true)] {
        let mut f = set_up_with(|config| {
            config.animations.window_resize.anim.off = window_resize_off;
            config.animations.off = all_off;
        });
        let id = f.add_client();

        let window = f.client(id).create_window();
        let surface = window.surface.clone();
        window.commit();
        f.roundtrip(id);
        let client = f.client(id);
        client
            .state
            .attach_shm_buffer(&surface, 960, 1080, [0, 0, 255]);
        client.window(&surface).ack_last_and_commit();
        f.double_roundtrip(id);
        render_output(&mut f, 1);

        f.niri().layout.toggle_full_width();
        f.double_roundtrip(id);

        let (_, mapped) = f.niri().layout.windows().next().unwrap();
        let off = window_resize_off || all_off;
        assert_eq!(mapped.has_configure_snapshot(), !off);
    }
}

#[test]
fn egl_resize_animation_after_moving_to_output_with_other_scale() {
    const FRAME: [u8; 3] = [0, 0, 255];
    const CONTENT: [u8; 3] = [0, 255, 0];

    // The window either moves and then gets an animated configure, or it moves before it answers
    // an animated configure that it got on the first output.
    for resize_before_move in [false, true] {
        let mut f = set_up_with(|config| {
            let outputs = Config::parse_mem(r#"output "headless-2" { scale 2; }"#).unwrap();
            config.outputs = outputs.outputs;
        });
        f.add_output(2, (1920, 1080));
        let id = f.add_client();

        // The left half of the window is the contents, the right half is the frame.
        let window = f.client(id).create_window();
        let surface = window.surface.clone();
        window.commit();
        f.roundtrip(id);
        let (content, _subsurface) = f.client(id).state.create_subsurface(&surface);

        let draw = |f: &mut Fixture, w: i32, h: i32| {
            let client = f.client(id);
            client.state.attach_shm_buffer(&content, w / 2, h, CONTENT);
            content.commit();
            client.state.attach_shm_buffer(&surface, w, h, FRAME);
            client.window(&surface).ack_last_and_commit();
            f.roundtrip(id);
        };

        draw(&mut f, 960, 1080);
        f.double_roundtrip(id);
        set_time(f.niri(), Duration::ZERO);
        f.niri_complete_animations();
        render_output(&mut f, 1);

        if resize_before_move {
            f.niri().layout.toggle_full_width();
            f.double_roundtrip(id);
        }

        // Move the window to the scale-2 output, where it becomes 480×540. That output doesn't
        // render before the window answers.
        let output = f.niri_output(2);
        f.niri()
            .layout
            .move_to_output(None, &output, None, ActivateWindow::Smart);
        f.double_roundtrip(id);
        draw(&mut f, 480, 540);

        // At the start of the animation, the window is still 960 logical pixels wide, which is
        // all of output 2, so its left half is the contents.
        let pixels = render_output(&mut f, 2);
        assert_eq!(pixel(&pixels, 720, 270), CONTENT, "{resize_before_move}");
        assert_eq!(pixel(&pixels, 1200, 270), FRAME, "{resize_before_move}");
    }
}

#[test]
fn egl_resize_animation_after_moving_to_a_larger_output_starts_from_the_whole_window() {
    // A 4744 px wide window on a 1920 px wide output gets an animated configure and moves to a
    // 2560 px wide output, at the same scale, before it answers. A texture rendered for the
    // snapshot when the configure was sent would be cut at twice the first output plus 500 px,
    // 4340 px. On the second output the window fits into twice the output plus 500 px, 5620 px, so
    // its resize goes through textures, and the starting texture must have all of the window.
    const CONTENT: [u8; 3] = [0, 255, 0];
    let mut f = set_up();
    f.add_output(2, (2560, 1440));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let client = f.client(id);
    client
        .state
        .attach_shm_buffer(&surface, 4744, 1000, CONTENT);
    client.window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);
    set_time(f.niri(), Duration::ZERO);
    f.niri_complete_animations();
    render_output(&mut f, 1);

    f.niri()
        .layout
        .set_column_width(SizeChange::SetProportion(100.));
    f.double_roundtrip(id);
    let mapped = f.niri().layout.windows().next().unwrap().1;
    assert!(mapped.has_configure_snapshot());

    // Output 2 doesn't render before the window answers.
    let output = f.niri_output(2);
    f.niri()
        .layout
        .move_to_output(None, &output, None, ActivateWindow::Smart);
    f.double_roundtrip(id);
    let client = f.client(id);
    client
        .state
        .attach_shm_buffer(&surface, 2528, 1000, CONTENT);
    client.window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);

    let output_size = f.niri_output(2).current_mode().unwrap().size;
    assert_eq!(output_size, (2560, 1440).into());
    render_output(&mut f, 2);
    let tile = f
        .niri()
        .layout
        .workspaces()
        .flat_map(|(_, _, ws)| ws.tiles())
        .next()
        .unwrap();
    assert!(tile.resize_animation().is_some());
    let geo = tile.resize_animation_texture_geo().unwrap();
    assert_eq!(geo.size.w, 4744, "{geo:?}");
}

#[test]
fn egl_open_animation_texture_stays_within_the_view() {
    // A window that doesn't set its window geometry is as large as its surfaces, which the client
    // places and sizes as it likes: here with a 1×1 subsurface far away, or with a large viewport
    // destination. The open animation renders the window into a texture, which must not follow.
    for far_subsurface in [true, false] {
        let mut f = set_up();
        set_time(f.niri(), Duration::ZERO);
        let id = f.add_client();

        let window = f.client(id).create_window();
        let surface = window.surface.clone();
        window.commit();
        f.roundtrip(id);

        let size = if far_subsurface {
            let (_far, subsurface) = f.client(id).state.create_subsurface(&surface);
            subsurface.set_position(5000, 5000);
            100
        } else {
            5000
        };
        let window = f.client(id).window(&surface);
        window.attach_new_buffer();
        window.set_size(size, size);
        window.ack_last_and_commit();
        f.roundtrip(id);

        render_output(&mut f, 1);

        let ws = f.niri().layout.active_workspace().unwrap();
        let tile = ws.tiles().next().unwrap();
        let window_size = tile.window_size();
        assert!(
            window_size.w >= 5000. && window_size.h >= 5000.,
            "{window_size:?}"
        );

        // At scale 1, the clip is the 1920 × 1080 view plus 500 px on each side, from (-500, -500)
        // relative to the tile. Nothing is drawn there before the 4 px focus ring at (-4, -4), so
        // the texture goes from there to the end of the clip.
        let texture_size = tile.open_animation_texture_size().unwrap();
        assert_eq!(
            texture_size,
            Size::from((2424, 1584)),
            "far_subsurface: {far_subsurface}"
        );
    }
}

#[test]
fn egl_snapshot_of_a_tile_dragged_in_the_overview_stays_within_the_view() {
    // A window that doesn't set its window geometry and has a 15000 × 15000 viewport destination,
    // dragged in the overview. The overview zooms out by 0.5, so the output shows 3840 × 2160 of
    // the dragged tile. Closing the window during the drag snapshots the tile with its fade, and
    // the fade's texture must stay within the 1920 × 1080 view plus 500 px on each side.
    let mut f = set_up();
    set_time(f.niri(), Duration::ZERO);
    let id = f.add_client();
    let _surface = create_window(&mut f, id, 15000, 15000);
    f.niri_complete_animations();
    render_output(&mut f, 1);

    f.niri().layout.toggle_overview();
    f.niri_complete_animations();
    assert_eq!(f.niri().layout.overview_zoom(), 0.5);

    let output = f.niri_output(1);
    let niri = f.niri();
    let window = niri.layout.windows().next().unwrap().1.window.clone();
    let start = Point::from((960., 540.));
    assert!(niri
        .layout
        .interactive_move_begin(window.clone(), &output, start));
    let delta = Point::from((200., 100.));
    niri.layout
        .interactive_move_update(&window, delta, output.clone(), start + delta);

    render_output(&mut f, 1);
    let tile = f.niri().layout.interactive_moved_tile().unwrap();
    let texture_size = tile.alpha_animation_texture_size().unwrap();
    assert!(
        texture_size.w <= 2920 && texture_size.h <= 2080,
        "drag: {texture_size:?}"
    );

    f.niri_state().store_unmap_snapshot(&window, Some(&output));
    let tile = f.niri().layout.interactive_moved_tile().unwrap();
    let texture_size = tile.alpha_animation_texture_size().unwrap();
    assert!(
        texture_size.w <= 2920 && texture_size.h <= 2080,
        "snapshot: {texture_size:?}"
    );
}

#[test]
fn egl_tile_dragged_in_the_overview_shows_what_the_output_shows() {
    // A 3792 × 1000 green tiled window, dragged in the overview so that its left edge is at
    // x = -600 of the output. The overview zooms it out by 0.5, so the output shows its tile x
    // from 1200 to 3796 (its end, with the 4 px focus ring), 2596 px, which fit into a fade
    // texture cut with a 1920 px view plus 500 px on each side. A view taken as if the tile weren't
    // zoomed would keep tile x 100 to 3020, shown up to x = -600 + 3020 / 2 = 910.
    const CONTENT: [u8; 3] = [0, 255, 0];
    let mut f = set_up();
    set_time(f.niri(), Duration::ZERO);
    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let client = f.client(id);
    client
        .state
        .attach_shm_buffer(&surface, 3792, 1000, CONTENT);
    client.window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);
    f.niri_complete_animations();

    f.niri().layout.toggle_overview();
    f.niri_complete_animations();
    assert_eq!(f.niri().layout.overview_zoom(), 0.5);

    // The tile is drawn with alpha 0.75 while it is dragged.
    let is_content = |[r, g, b]: [u8; 3]| g > 150 && r < 60 && b < 60;
    let pixels = render_output(&mut f, 1);
    let left = (0..1920)
        .find(|&x| is_content(pixel(&pixels, x, 540)))
        .unwrap();

    // Grab the tile 1000 px right of its left edge and drag it so that the edge is at x = -600.
    let output = f.niri_output(1);
    let niri = f.niri();
    let window = niri.layout.windows().next().unwrap().1.window.clone();
    let start = Point::from((left as f64 + 1000., 540.));
    assert!(niri
        .layout
        .interactive_move_begin(window.clone(), &output, start));
    let delta = Point::from((400. - start.x, 0.));
    niri.layout
        .interactive_move_update(&window, delta, output, start + delta);
    // Let the tile catch up with the pointer and fade to semitransparent.
    set_time(f.niri(), Duration::from_secs(10));

    let pixels = render_output(&mut f, 1);
    assert!(f.niri().layout.interactive_moved_tile().is_some());
    let row: Vec<_> = (0..1920)
        .map(|x| is_content(pixel(&pixels, x, 540)))
        .collect();
    let last = row.iter().rposition(|&c| c).unwrap();
    assert!((1290..=1300).contains(&last), "{last}");
    for x in [0, 1000, 1200, 1280] {
        assert!(row[x], "x = {x}: {:?}", pixel(&pixels, x, 540));
    }
}

#[test]
fn egl_close_during_a_drag_in_the_overview_keeps_what_the_closing_animation_shows() {
    // A 4744 × 1000 green tiled window, dragged in the overview so that its left edge is at
    // x = -600 of the output, closes during the drag. The closing animation draws the tile in the
    // workspace under the pointer, which starts at x = 480 and is zoomed out by 0.5, at the tile's
    // position relative to it, x = -600 - 480 = -1080, and with the workspace's 1920 px view. So
    // the output shows tile x from 120 on, and the closing animation keeps tile x 580 to 3500
    // (the view plus 500 px on each side), shown from x = -60 + 580 / 2 = 230 to 1690. A snapshot
    // cut around what the drag showed (tile x 1200 to 4748) would keep only tile x 1514 to 4434,
    // shown from x = 697.
    const CONTENT: [u8; 3] = [0, 255, 0];
    let mut f = set_up_with(|config| {
        config.animations.window_close.anim.kind = Kind::Easing(EasingParams {
            duration_ms: 1000,
            curve: Curve::Linear,
        });
    });
    set_time(f.niri(), Duration::ZERO);
    let id = f.add_client();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let client = f.client(id);
    client
        .state
        .attach_shm_buffer(&surface, 4744, 1000, CONTENT);
    client.window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);
    f.niri_complete_animations();

    f.niri().layout.toggle_overview();
    f.niri_complete_animations();
    assert_eq!(f.niri().layout.overview_zoom(), 0.5);

    let is_content = |[r, g, b]: [u8; 3]| g > 150 && r < 60 && b < 60;
    let pixels = render_output(&mut f, 1);
    let left = (0..1920)
        .find(|&x| is_content(pixel(&pixels, x, 540)))
        .unwrap();
    assert_eq!(left, 480);

    // Grab the tile 1000 px right of its left edge and drag it so that the edge is at x = -600.
    let output = f.niri_output(1);
    let niri = f.niri();
    let window = niri.layout.windows().next().unwrap().1.window.clone();
    let start = Point::from((left as f64 + 1000., 540.));
    assert!(niri
        .layout
        .interactive_move_begin(window.clone(), &output, start));
    let delta = Point::from((400. - start.x, 0.));
    niri.layout
        .interactive_move_update(&window, delta, output, start + delta);
    set_time(f.niri(), Duration::from_secs(10));
    render_output(&mut f, 1);

    let window = f.client(id).window(&surface);
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.surface.destroy();
    f.double_roundtrip(id);
    assert!(f.niri().layout.interactive_moved_tile().is_none());

    // Right after the start of the closing animation.
    set_time(f.niri(), Duration::from_millis(10_010));
    let pixels = render_output(&mut f, 1);
    let column_has_content = |x: usize| (0..1080).any(|y| is_content(pixel(&pixels, x, y)));
    for x in [300, 600, 1000, 1600] {
        assert!(column_has_content(x), "x = {x}");
    }
}

// Sets up a fixture with linear open and close animations where windows open floating, and opens
// a 5000 × 100 window of the CONTENT color at x = -2000, so the view shows its middle, more than
// 500 px away from both of its sides. Opening starts at time zero.
fn set_up_wide_window(content: [u8; 3]) -> (Fixture, ClientId, WlSurface) {
    let mut f = set_up_with(|config| {
        const LINEAR: Kind = Kind::Easing(EasingParams {
            duration_ms: 1000,
            curve: Curve::Linear,
        });
        config.animations.window_open.anim.kind = LINEAR;
        config.animations.window_close.anim.kind = LINEAR;
        config.window_rules.push(niri_config::WindowRule {
            open_floating: Some(true),
            ..Default::default()
        });
    });
    set_time(f.niri(), Duration::ZERO);
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let client = f.client(id);
    client.state.attach_shm_buffer(&surface, 5000, 100, content);
    client.window(&surface).ack_last_and_commit();
    f.roundtrip(id);

    let x = PositionChange::SetFixed(-2000.);
    let y = PositionChange::AdjustFixed(0.);
    f.niri().layout.move_floating_window(None, x, y, false);

    (f, id, surface)
}

#[test]
fn egl_open_animation_shows_the_middle_of_a_window_wider_than_the_view() {
    const CONTENT: [u8; 3] = [0, 255, 0];
    let (mut f, _id, _surface) = set_up_wide_window(CONTENT);

    // Almost at the end of the open animation, the window is almost opaque and full-size.
    set_time(f.niri(), Duration::from_millis(990));
    let pixels = render_output(&mut f, 1);

    let ws = f.niri().layout.active_workspace().unwrap();
    let (tile, pos, _) = ws.tiles_with_render_positions().next().unwrap();
    assert!(tile.open_animation_texture_size().is_some());
    assert_eq!(pos.x, -2000.);

    let [r, g, b] = pixel(&pixels, 960, (pos.y + 50.) as usize);
    assert!(r < 10 && g > 240 && b < 10, "{:?}", [r, g, b]);
}

#[test]
fn egl_close_animation_shows_the_middle_of_a_window_wider_than_the_view() {
    const CONTENT: [u8; 3] = [0, 255, 0];
    let (mut f, id, surface) = set_up_wide_window(CONTENT);

    set_time(f.niri(), Duration::from_secs(2));
    f.niri_complete_animations();
    render_output(&mut f, 1);
    let ws = f.niri().layout.active_workspace().unwrap();
    let (tile, pos, _) = ws.tiles_with_render_positions().next().unwrap();
    assert!(tile.open_animation_texture_size().is_none());
    assert_eq!(pos.x, -2000.);
    let y = (pos.y + 50.) as usize;

    let window = f.client(id).window(&surface);
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.surface.destroy();
    f.double_roundtrip(id);

    // Right after the start of the close animation, the window is almost opaque and full-size.
    set_time(f.niri(), Duration::from_millis(2010));
    let pixels = render_output(&mut f, 1);
    assert!(f
        .niri()
        .layout
        .active_workspace()
        .unwrap()
        .tiles()
        .next()
        .is_none());

    let [r, g, b] = pixel(&pixels, 960, y);
    assert!(r < 10 && g > 240 && b < 10, "{:?}", [r, g, b]);
}

#[test]
fn egl_close_during_open_animation_shows_the_middle_of_a_window_wider_than_the_view() {
    const CONTENT: [u8; 3] = [0, 255, 0];
    let (mut f, id, surface) = set_up_wide_window(CONTENT);

    // Halfway through the open animation.
    set_time(f.niri(), Duration::from_millis(500));
    let pixels = render_output(&mut f, 1);
    let ws = f.niri().layout.active_workspace().unwrap();
    let (tile, pos, _) = ws.tiles_with_render_positions().next().unwrap();
    assert!(tile.open_animation_texture_size().is_some());
    assert_eq!(pos.x, -2000.);
    let y = (pos.y + 50.) as usize;
    let opening = pixel(&pixels, 960, y);

    let window = f.client(id).window(&surface);
    window.xdg_toplevel.destroy();
    window.xdg_surface.destroy();
    window.surface.destroy();
    f.double_roundtrip(id);

    // The snapshot for the close animation is taken during the open animation. Right after the
    // start of the close animation, the middle of the view still shows the window as it was.
    set_time(f.niri(), Duration::from_millis(510));
    let pixels = render_output(&mut f, 1);
    assert!(f
        .niri()
        .layout
        .active_workspace()
        .unwrap()
        .tiles()
        .next()
        .is_none());

    let closing = pixel(&pixels, 960, y);
    let close = |a: u8, b: u8| a.abs_diff(b) <= 4;
    assert!(
        opening[1] > opening[0] + 100 && zip(opening, closing).all(|(a, b)| close(a, b)),
        "opening: {opening:?}, closing: {closing:?}"
    );
}

/// Opens a 960 px wide window of the CONTENT color, resizes its column to `percent` of the view
/// with a linear 1000 ms animation, and returns the pixels at `ms` into the animation.
fn render_resize_of_wide_window(content: [u8; 3], percent: f64, ms: u64) -> Vec<u8> {
    let mut f = set_up();
    set_time(f.niri(), Duration::ZERO);
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let client = f.client(id);
    client.state.attach_shm_buffer(&surface, 960, 1080, content);
    client.window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);
    f.niri_complete_animations();
    render_output(&mut f, 1);

    f.niri()
        .layout
        .set_column_width(SizeChange::SetProportion(percent));
    f.double_roundtrip(id);
    let width = (1920. * percent / 100.) as i32;
    let client = f.client(id);
    client
        .state
        .attach_shm_buffer(&surface, width, 1080, content);
    client.window(&surface).ack_last_and_commit();
    f.double_roundtrip(id);

    set_time(f.niri(), Duration::from_millis(ms));
    let pixels = render_output(&mut f, 1);
    let ws = f.niri().layout.active_workspace().unwrap();
    let tile = ws.tiles().next().unwrap();
    assert!(tile.resize_animation().is_some());
    let expected_w = 960. + (f64::from(width) - 960.) * ms as f64 / 1000.;
    assert_eq!(tile.animated_tile_size().w, expected_w);
    pixels
}

#[test]
fn egl_resize_animation_of_a_window_twice_as_wide_as_the_view() {
    // Halfway through, the tile is 2400 px wide. The resize shader stretches the 3840 px window
    // over it, so x = 1800 shows x = 2880 of the window, which a texture cut to the view plus
    // 500 px wouldn't have. Twice the view still goes through whole textures.
    const CONTENT: [u8; 3] = [0, 255, 0];
    let pixels = render_resize_of_wide_window(CONTENT, 200., 500);
    assert_eq!(pixel(&pixels, 1800, 540), CONTENT);
}

#[test]
fn egl_resize_animation_draws_a_window_wider_than_twice_the_view_as_is() {
    // At 200 ms, the tile is 1920 px wide. The resize shader would stretch the 5760 px window
    // over it, so x = 1800 would show x = 5400 of the window, past twice the view plus 500 px.
    // The window is drawn as is instead.
    const CONTENT: [u8; 3] = [0, 255, 0];
    let pixels = render_resize_of_wide_window(CONTENT, 300., 200);
    assert_eq!(pixel(&pixels, 1800, 540), CONTENT);
}
