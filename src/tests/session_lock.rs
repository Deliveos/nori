use std::sync::atomic::Ordering;

use niri_config::Action;
use smithay::backend::input::InputTime;

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

    let (_lock, data) = f.client(id).lock_session();
    for _ in 0..100 {
        f.double_roundtrip(id);
        if data.locked.load(Ordering::Relaxed) {
            break;
        }
    }
    assert!(data.locked.load(Ordering::Relaxed));
    assert!(f.niri().is_locked());

    // The frames that confirmed the lock didn't show the transition.
    assert!(!f.niri().has_screen_transition());
}
