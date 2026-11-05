use smithay::backend::input::{ButtonState, InputTime};
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::utils::{Point, SERIAL_COUNTER};
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::tests::client::ClientId;

/// Maps a window for the client and returns its surface.
fn map_window(f: &mut Fixture, id: ClientId) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    surface
}

/// Presses the left button on the only mapped window and returns the serial of the press.
fn press_on_window(f: &mut Fixture) -> u32 {
    let surfaces: Vec<_> = f
        .niri()
        .layout
        .windows()
        .map(|(_, mapped)| mapped.toplevel().wl_surface().clone())
        .collect();
    let [surface] = &surfaces[..] else {
        panic!("expected one window, got {}", surfaces.len());
    };
    let surface = surface.clone();

    let state = f.niri_state();
    let pointer = state.niri.seat.get_pointer().unwrap();
    let event = MotionEvent {
        location: Point::from((1., 1.)),
        serial: SERIAL_COUNTER.next_serial(),
        time: InputTime::now(),
    };
    pointer.motion(state, Some((surface, Point::from((0., 0.)))), &event);
    pointer.frame(state);

    let serial = SERIAL_COUNTER.next_serial();
    let event = ButtonEvent {
        serial,
        time: InputTime::now(),
        button: 0x110, // BTN_LEFT
        state: ButtonState::Pressed,
    };
    pointer.button(state, &event);
    pointer.frame(state);

    serial.into()
}

/// Sends `start_drag` from the client with an icon, so that a started drag shows up as the DnD
/// icon.
fn start_drag(f: &mut Fixture, id: ClientId, origin: &WlSurface, serial: u32) {
    let manager = f.client(id).bind_data_device_manager();
    let state = &mut f.client(id).state;
    let seat = state.seats.keys().next().unwrap().clone();
    let qh = state.qh.clone();
    let device = manager.get_data_device(&seat, &qh, ());
    let source = manager.create_data_source(&qh, ());
    source.offer(String::from("text/plain"));
    let icon = state.create_surface();
    device.start_drag(Some(&source), origin, Some(&icon), serial);
    f.double_roundtrip(id);
}

#[test]
fn drag_starts_on_own_click() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();

    let surface = map_window(&mut f, id);
    let serial = press_on_window(&mut f);
    start_drag(&mut f, id, &surface, serial);

    assert!(f.niri().dnd_icon.is_some());
}

// #153: serials come from one counter for all clients, so a client can guess the serial of a
// click on another client's window and take over that click as a drag with its own data.
#[test]
fn drag_does_not_start_on_another_clients_click() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let attacker = f.add_client();
    let victim = f.add_client();

    let victim_surface = map_window(&mut f, victim);
    let serial = press_on_window(&mut f);

    // The attacker sends the serial of the victim's click, as if it had guessed it.
    let origin = f.client(attacker).state.create_surface();
    start_drag(&mut f, attacker, &origin, serial);
    assert!(
        f.niri().dnd_icon.is_none(),
        "a client started a drag on another client's click"
    );

    // The click is still there for the client that got it.
    start_drag(&mut f, victim, &victim_surface, serial);
    assert!(f.niri().dnd_icon.is_some());
}
