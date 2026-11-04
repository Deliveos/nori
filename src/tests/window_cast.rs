use smithay::reexports::wayland_protocols::xdg::shell::client::xdg_positioner::Gravity;
use smithay::utils::{Logical, Physical, Rectangle, Scale, Size};

use super::*;
use crate::utils::output_size;

/// Maps a 100 × 100 window with a subsurface at (16000, 16000) on a 1920 × 1080 output, and
/// returns its window geometry and its cast area.
fn far_subsurface_cast_bbox(
    set_geometry: bool,
) -> (Rectangle<i32, Logical>, Rectangle<i32, Physical>) {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let (_, subsurface) = f.client(id).state.create_subsurface(&surface);
    subsurface.set_position(16000, 16000);
    let window = f.client(id).window(&surface);
    if set_geometry {
        window.xdg_surface.set_window_geometry(0, 0, 100, 100);
    }
    window.commit();
    f.double_roundtrip(id);

    let output = f.niri_output(1);
    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    assert!(mapped.window.bbox_with_popups().size.w > 16000);

    let geo = mapped.window.geometry();
    let bbox = mapped.cast_bbox(Scale::from(1.), output_size(&output), &mut None);
    (geo, bbox)
}

#[test]
fn window_cast_area_is_cropped_around_the_window() {
    let (geo, bbox) = far_subsurface_cast_bbox(true);
    assert_eq!(geo, Rectangle::from_size(Size::from((100, 100))));

    // The cast is at most the output's size, and starts at the window.
    assert_eq!(bbox, Rectangle::from_size(Size::from((1920, 1080))));
}

#[test]
fn window_cast_area_is_cropped_without_window_geometry() {
    // Without a window geometry, the geometry is the bounding box of the surface and its
    // subsurfaces, so it reaches the far subsurface.
    let (geo, bbox) = far_subsurface_cast_bbox(false);
    assert_eq!(geo, Rectangle::from_size(Size::from((16001, 16001))));

    // The cast is still at most the output's size, from the start of the window geometry.
    assert_eq!(bbox, Rectangle::from_size(Size::from((1920, 1080))));
}

#[test]
fn window_cast_area_keeps_room_for_a_closed_popup() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.xdg_surface.set_window_geometry(0, 0, 100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let output = f.niri_output(1);
    let mut held = None;
    let mut cast_bbox = |f: &mut Fixture| {
        let (_, mapped) = f.niri().layout.windows().next().unwrap();
        mapped.cast_bbox(Scale::from(1.), output_size(&output), &mut held)
    };
    let window_only = Rectangle::from_size(Size::from((100, 100)));
    assert_eq!(cast_bbox(&mut f), window_only);

    // A popup above and left of the window makes the area larger.
    let xdg_surface = f.client(id).window(&surface).xdg_surface.clone();
    let (popup_surface, popup_xdg_surface, popup) = f
        .client(id)
        .state
        .create_popup(&xdg_surface, Gravity::TopLeft);
    f.roundtrip(id);
    f.client(id).state.attach_pixel_buffer(&popup_surface);
    popup_surface.commit();
    f.double_roundtrip(id);
    let with_popup = cast_bbox(&mut f);
    assert!(
        with_popup.contains_rect(window_only) && with_popup != window_only,
        "{with_popup:?}"
    );

    // When the popup closes, the area stays.
    popup.destroy();
    popup_xdg_surface.destroy();
    popup_surface.destroy();
    f.double_roundtrip(id);
    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    let logical: Rectangle<i32, Logical> = Rectangle::from_size(Size::from((100, 100)));
    assert_eq!(mapped.window.bbox_with_popups(), logical);
    assert_eq!(cast_bbox(&mut f), with_popup);

    // Until the window geometry changes.
    let window = f.client(id).window(&surface);
    window.xdg_surface.set_window_geometry(0, 0, 80, 80);
    window.commit();
    f.double_roundtrip(id);
    assert_eq!(cast_bbox(&mut f), window_only);
}

#[test]
fn window_cast_area_held_room_doesnt_crop_what_is_shown_now() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.xdg_surface.set_window_geometry(0, 0, 100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let output = f.niri_output(1);
    let mut held = None;
    let mut cast_bbox = |f: &mut Fixture| {
        let (_, mapped) = f.niri().layout.windows().next().unwrap();
        mapped.cast_bbox(Scale::from(1.), output_size(&output), &mut held)
    };
    let rect = |x, w| Rectangle::new((x, 0).into(), (w, 100).into());

    // A subsurface 1500 px left of the window fits into the output with it.
    let (left_surface, left) = f.client(id).state.create_subsurface(&surface);
    left.set_position(-1500, 0);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(cast_bbox(&mut f), rect(-1500, 1600));

    // It goes away, and another one appears 1500 px right of the window. Together with the room
    // held for the first one, that's more than the output, and the new one must still be cast.
    left.destroy();
    left_surface.destroy();
    let (_right_surface, right) = f.client(id).state.create_subsurface(&surface);
    right.set_position(1500, 0);
    surface.commit();
    f.double_roundtrip(id);
    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    let logical: Rectangle<i32, Logical> = Rectangle::from_size(Size::from((1501, 100)));
    assert_eq!(mapped.window.bbox_with_popups(), logical);
    assert_eq!(cast_bbox(&mut f), rect(-419, 1920));
}

#[test]
fn window_cast_area_holds_its_place_and_physical_size() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.xdg_surface.set_window_geometry(0, 0, 100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // As for an output 2880 × 1620 px at scale 1.5.
    let scale = Scale::from(1.5);
    let max_size = Size::from((1920., 1080.));
    let mut held = None;
    let mut cast_bbox = |f: &mut Fixture| {
        let (_, mapped) = f.niri().layout.windows().next().unwrap();
        mapped.cast_bbox(scale, max_size, &mut held)
    };
    let rect = |x, w, h| Rectangle::new((x, 0).into(), (w, h).into());

    // The same steps as in the test above: the held area is cropped to (-419, 1920) logical, which
    // rounds out to x from -629 to 2252.
    let (left_surface, left) = f.client(id).state.create_subsurface(&surface);
    left.set_position(-1500, 0);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(cast_bbox(&mut f), rect(-2250, 2400, 150));

    left.destroy();
    left_surface.destroy();
    let (right_surface, right) = f.client(id).state.create_subsurface(&surface);
    right.set_position(1500, 0);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(cast_bbox(&mut f), rect(-629, 2881, 150));

    // When the right one goes away too, the area stays where it was, because it still holds the
    // window. Cropped around the window alone, it would be (-910, 1920) logical.
    right.destroy();
    right_surface.destroy();
    surface.commit();
    f.double_roundtrip(id);
    let (_, mapped) = f.niri().layout.windows().next().unwrap();
    let logical: Rectangle<i32, Logical> = Rectangle::from_size(Size::from((100, 100)));
    assert_eq!(mapped.window.bbox_with_popups(), logical);
    assert_eq!(cast_bbox(&mut f), rect(-629, 2881, 150));

    // A subsurface below the window makes the area taller, 201 logical px, 302 px. Across, the
    // area stays where it was and keeps its 2881 px.
    let (_below_surface, below) = f.client(id).state.create_subsurface(&surface);
    below.set_position(0, 200);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(cast_bbox(&mut f), rect(-629, 2881, 302));

    // A subsurface outside that area, and further down, moves it to (-1500, 1920) logical and makes
    // it 301 logical px tall, 452 px. Across, that rounds out to x from -2250 to 630, 2880 px, but
    // the stream keeps its 2881 px, also while the height changes.
    let (_far_surface, far) = f.client(id).state.create_subsurface(&surface);
    far.set_position(-1300, 300);
    surface.commit();
    f.double_roundtrip(id);
    assert_eq!(cast_bbox(&mut f), rect(-2250, 2881, 452));
}
