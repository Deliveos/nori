use std::sync::atomic::Ordering;

use niri_config::Action;
use smithay::backend::input::InputTime;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::ExtSessionLockV1;

use super::client::{ClientId, ImageCopyFrameResult};
use super::*;

#[test]
fn egl_screen_transition_is_dropped_when_locking() {
    let mut f = Fixture::new();
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (640, 480));
    let id = f.add_client();

    // Like `do-screen-transition --delay-ms 10000` right before starting a lock screen. The
    // transition is a snapshot of the unlocked desktop.
    f.niri_state().do_action(
        Action::DoScreenTransition(Some(10000)),
        false,
        InputTime::now(),
    );
    assert!(f.niri().has_screen_transition());

    let _lock = lock(&mut f, id);

    // The frames that confirmed the lock didn't show the transition.
    assert!(!f.niri().has_screen_transition());
}

fn lock(f: &mut Fixture, id: ClientId) -> ExtSessionLockV1 {
    let (lock, data) = f.client(id).lock_session();
    for _ in 0..100 {
        f.double_roundtrip(id);
        if data.locked.load(Ordering::Relaxed) {
            break;
        }
    }
    assert!(data.locked.load(Ordering::Relaxed));
    assert!(f.niri().is_locked());
    lock
}

#[test]
fn egl_window_capture_frames_wait_while_locked() {
    let mut f = Fixture::new();
    f.niri_state().backend.headless().add_renderer().unwrap();
    f.add_output(1, (640, 480));
    let id = f.add_client();

    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.set_title("captured");
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(100, 100);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let client = f.client(id);
    let toplevel = client.state.foreign_toplevels[0].handle.clone();
    let (session, data) = client.capture_toplevel(&toplevel);
    f.double_roundtrip(id);

    let lock = lock(&mut f, id);

    // The first frame of a session has full damage, so it would be captured on the next redraw.
    let frame = f.client(id).capture_frame(&session, &data);
    for _ in 0..10 {
        f.double_roundtrip(id);
    }
    assert_eq!(*frame.lock().unwrap(), None);

    lock.unlock_and_destroy();
    for _ in 0..100 {
        f.double_roundtrip(id);
        if frame.lock().unwrap().is_some() {
            break;
        }
    }
    assert_eq!(*frame.lock().unwrap(), Some(ImageCopyFrameResult::Ready));
}
