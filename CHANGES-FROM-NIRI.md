# How nori differs from niri

nori is a fork of [niri](https://github.com/niri-wm/niri).
This file lists what nori does differently from upstream niri: crash and freeze fixes, security fixes, other bug fixes, performance and memory work, and the changes that users and packagers notice.
Each entry links to the pull requests or commits that made the change and to the issues that describe the problem.

Pull requests that change how nori behaves compared with niri update this file in the same pull request (see [CONTRIBUTING.md](CONTRIBUTING.md#writing-pull-requests)).

- **Upstream base:** nori contains upstream niri up to [a8562a1](https://github.com/niri-wm/niri/commit/a8562a1126922ed4c1c94d34c1dbb26633491d05), merged in [#268](https://github.com/Deliveos/nori/pull/268).
    Later upstream commits are not in nori until the next merge (see [Upstream merges](#upstream-merges)).
- **Older work:** the first fixes were made in an earlier repository and moved here in [#1](https://github.com/Deliveos/nori/pull/1).
    Their commit messages use that repository's issue and pull request numbers ("Fixes #N", "Merge pull request #N"), which don't match the numbers here.
    This file links those changes by commit and gives each issue the number it has in this repository.

## Contents

- [What you notice coming from niri](#what-you-notice-coming-from-niri)
- [Names](#names)
- [Crash fixes](#crash-fixes)
- [Freeze and hang fixes](#freeze-and-hang-fixes)
- [Security and the lock screen](#security-and-the-lock-screen)
- [Other bug fixes](#other-bug-fixes)
- [Performance](#performance)
- [Memory](#memory)
- [Building and packaging](#building-and-packaging)
- [Tests and CI](#tests-and-ci)
- [Project rules](#project-rules)
- [Upstream merges](#upstream-merges)
- [Open problems](#open-problems)

## What you notice coming from niri

- The command is `nori`, the config is `~/.config/nori/config.kdl`, and the IPC socket path is in `$NORI_SOCKET`; see [Names](#names).
    nori doesn't read `~/.config/niri`.
- smithay is built from the [Deliveos/smithay](https://github.com/Deliveos/smithay) fork; see [Building and packaging](#building-and-packaging).
- Limits that niri doesn't have:
    - an IPC request line may be at most 8 MiB ([details](#ipc-request-limit));
    - at most 16 pastes of nori's own clipboard data (such as a copied screenshot) can be pending at once ([details](#clipboard-writer-threads));
    - a blur region with more than 64 rectangles is blurred over its bounding box ([details](#blur-region-cap));
    - at most 16 wlr-gamma-control ramps can be read at once ([details](#gamma-reads));
    - configured output positions are clamped to ±16777216, and custom modes with zero timings are ignored ([details](#output-positions-and-custom-modes));
    - a window screencast covers at most an area the size of the window's output (rounded up to whole logical pixels, then out to whole physical pixels, so on each axis it can be larger than the output by fewer than the scale plus 2 physical pixels), also when the window itself is larger, and whatever lies outside that area is not cast ([details](#window-cast-size));
    - DisplayConfig rejects scales that are not finite and positive ([details](#displayconfig-scale));
    - at most 4 screenshots can be saved at once ([details](#screenshot-threads));
    - the xwayland-satellite on-demand test must finish within 2 seconds, or the integration stays off until a config reload changes the `xwayland-satellite` section, or a restart ([details](#xwayland-satellite-test)).
- Screenshots named from `screenshot-path` never overwrite an existing file: when the name is taken (two screenshots in the same second, or a `screenshot-path` without time fields), nori adds ` (2)` … ` (999)`, and saving fails after that; with `screenshot-path null` D-Bus screenshots go to `$XDG_RUNTIME_DIR/screenshot.png` instead of `/tmp/screenshot.png` ([details](#screenshots)).
- Only the owner of the `org.gnome.Orca.KeyboardMonitor` bus name may listen to or grab keys through the accessibility D-Bus interface ([details](#keyboardmonitor)).
- PipeWire screencasts offer 4 buffers by default instead of 8 ([details](#pipewire-buffers)).
- Starting the session with `--session` on systemd stops an `xdg-desktop-portal.service` that is already running, so it restarts with nori's environment ([details](#portal-at-session-start)).

## Names

nori renames the names that users and other programs see ([#78](https://github.com/Deliveos/nori/pull/78); fixes [#76](https://github.com/Deliveos/nori/issues/76), [#77](https://github.com/Deliveos/nori/issues/77)).

- Commands: the binary is `nori` (`nori msg`, `nori validate`, and shell completions for `nori`). No `niri` binary is built.
- Config: `$XDG_CONFIG_HOME/nori/config.kdl` (usually `~/.config/nori/config.kdl`) and `/etc/nori/config.kdl`.
    There is no fallback to the niri paths: with neither file present, nori writes a default config to the user path.
- Environment: `NORI_SOCKET`, `NORI_CONFIG` and `NORI_DISABLE_SYSTEM_MANAGER_NOTIFY`; for builds, `NORI_BUILD_VERSION_STRING` and `NORI_BUILD_COMMIT`.
    Tools that read `NIRI_SOCKET`, including ones built on upstream's `niri-ipc` crate, don't find the socket until they are updated.
- The IPC socket is `nori.<wayland display>.<pid>.sock`, `XDG_CURRENT_DESKTOP` is `nori` (so xdg-desktop-portal reads `nori-portals.conf`), and spawned programs run in `app-nori-*` systemd scopes.
- Session files: `nori-session`, `nori.service`, `nori-shutdown.target`, `nori.desktop` (shown as "Nori"), and the dinit services `nori` and `nori.target`.
- Packaging: `nori.spec.rpkg`, the Nix flake packages `nori` and `nori-debug`, the Cargo RPM and deb metadata, and the release asset `nori-<version>-vendored-dependencies.tar.xz`.
- Internal names stay niri to keep upstream merges simple: the crates (`niri`, `niri-config`, `niri-ipc`, `niri-visual-tests`), source paths and types, the `niri` log target of library messages (startup messages of the binary use `nori`), the `niri_*` uniforms of custom shaders, and the pages in `docs/wiki/`.
    The dynamic cast target also still reports the app ID `rs.bxt.niri.desktop` to the portal's window picker; only its title says nori.
    [README.md](README.md) explains how to read the upstream documentation.

## Crash fixes

A crash of the compositor ends the whole session.

- **Negative sizes in the layout.**
    niri builds the insert hint of a new column with the working area height minus two gaps, and the edges and corners of a window's border from the animated tile size minus twice the border width, without a lower bound.
    The first is negative when the working area is shorter than two gaps (large `gaps`, `struts` or a layer-shell exclusive zone), the second during a resize animation that overshoots below twice the border width, for example a spring with a low damping ratio; with uneven corner radii (such as `geometry-corner-radius 29 0 0 0`), rounding the fitted radius up can also make one corner larger than its side and the next one negative.
    For the insert hint, and for a border drawn without the window background (server-side decorations or `draw-border-with-background false`), these sizes fail a debug assertion in smithay, so builds with debug assertions (the dev profile and `cargo test`) panic; what release builds draw then was not checked.
    nori clamps the insert hint height and the sizes of the border's edges and corners to 0, and has tests for both in `src/layout/tests.rs` ([#213](https://github.com/Deliveos/nori/pull/213); fixes [#176](https://github.com/Deliveos/nori/issues/176)).
- <a id="output-positions-and-custom-modes"></a>**Huge output positions and zero-size custom modes.**
    In niri, a very large output position (from the config, IPC or wlr-output-management) overflows `i32` when the next output is placed, and a custom mode such as width 0 gives zero timings that smithay divides by.
    nori clamps configured positions to ±2^24 (16777216), skips custom modes whose clock, htotal or vtotal is zero (with the warning "ignoring invalid custom mode"), and treats refresh intervals that the frame clock can't use as unknown ([71f4f158](https://github.com/Deliveos/nori/commit/71f4f158ee8a8ec87d52ee7ca9f72e2ce9b8a9f3); fixes [#15](https://github.com/Deliveos/nori/issues/15)).
- **NUL bytes in config strings.**
    KDL strings can contain a NUL byte (`"\u{0}"`), and niri passes the xkb `rules`, `model`, `layout` and `variant`, the `xcursor-theme` and workspace names on to code that panics on one (xkbcommon-rs, `std::env::set_var()`, and the Wayland message code for ext-workspace); `niri validate` accepts such a config.
    nori rejects a NUL byte in workspace names when parsing the config, ignores `set-workspace-name` with one (an IPC request gets an error), treats such xkb names like a keymap that fails to compile, and unsets `XCURSOR_THEME` for such a theme name.
    The tests are `nul_bytes_in_xkb_names_and_cursor_theme` in `src/tests/crash_regressions.rs`, `workspace_name_with_nul_is_ignored` in `src/layout/tests.rs`, `workspace_name_with_nul_is_refused` in `src/ipc/server.rs` and `parse_workspace_name_with_null_byte` in `niri-config` ([#204](https://github.com/Deliveos/nori/pull/204); fixes [#184](https://github.com/Deliveos/nori/issues/184)).
- **Workspaces on removed or unfocused monitors.**
    In niri, an ext-workspace assign to an output that was removed meanwhile panics, and naming a workspace on an unfocused monitor (by id or name over IPC) checks the focused monitor instead of the workspace's own, so the workspace's monitor doesn't get the empty workspace it needs, and a later cleanup can trip an assert.
    nori ignores assigns to outputs that no longer exist and uses the monitor that holds the workspace ([ce714900](https://github.com/Deliveos/nori/commit/ce71490003636727ec1ec79a0dfa086caba67c65); fixes [#16](https://github.com/Deliveos/nori/issues/16)).
- **Valid spring animation configs.**
    In niri, a bouncy workspace-switch spring (for example `damping-ratio=0.2`) can overshoot so that no workspace is on screen, and an interactive window move then panics; an overdamped spring with a fast initial velocity makes the spring duration negative and panics.
    nori doesn't cull in that window-move code and falls back to an estimate when the computed duration is invalid ([49b38eec](https://github.com/Deliveos/nori/commit/49b38eec3f0ec501a1b7ba23e48aad2e3db32df0); fixes [#11](https://github.com/Deliveos/nori/issues/11)).
    Not culling there let the insert hint point at a workspace that is off screen, and drawing the hint then panicked, for example when the output got smaller (a scale, mode or rotation change) in the middle of a move; nori doesn't draw the hint in that case, and has a test for it in `src/layout/tests.rs`. Unlike niri, when the output's scale or transform changes in the middle of a move, nori also updates the window being moved right away (its border, focus ring and pixel rounding, and the preferred buffer scale and transform sent to the client), instead of keeping the old ones until the window moves to another output or is dropped; `interactive_move_follows_output_scale_change` in `src/tests/floating.rs` tests the preferred buffer scale ([#198](https://github.com/Deliveos/nori/pull/198); fixes [#145](https://github.com/Deliveos/nori/issues/145)).
- **Left click in the window switcher with the pointer on no output** (after an unplug, an output turned off, or a scale change) panics in niri.
    nori treats it as a click outside the switcher and closes it ([b27a9740](https://github.com/Deliveos/nori/commit/b27a9740a63aaf9b8e1078be4cafe9a6ab76e89f); fixes [#10](https://github.com/Deliveos/nori/issues/10)).
- **Toggling floating during a drag after the dragged window closed.**
    In niri, if the window being moved closes, and the user presses the other mouse button or puts down a second finger (which toggles floating during a move) before the next pointer or touch motion, the toggle looks for a window that is gone and panics.
    nori ignores the toggle in that case, and has a test for it in `src/layout/tests.rs`; the randomized layout tests now also toggle floating for windows that are gone ([#199](https://github.com/Deliveos/nori/pull/199); fixes [#146](https://github.com/Deliveos/nori/issues/146)).
- <a id="clipboard-writer-threads"></a>**Clipboard writer threads.**
    When niri itself owns the clipboard (a screenshot copied to the clipboard), it starts one thread per paste request, and each thread blocks until the client reads.
    A client that never reads piles up threads (200 requests left 200 threads in the issue's test), and when starting a thread fails, niri panics.
    nori allows at most 16 such transfers at once, drops further requests with the warning "too many pending selection writes, dropping request", and logs a failed thread start instead of panicking.
    The limit is global, so while one client holds 16 unread transfers, pastes of nori's own clipboard data fail for all clients (the request is dropped and the client reads nothing) until that client reads or closes them ([44a1124e](https://github.com/Deliveos/nori/commit/44a1124e8b3ed61087ffe4ffc20e99537236126f); fixes [#9](https://github.com/Deliveos/nori/issues/9)).
- **Tablet mapped to a window that is barely visible.**
    With `map-to-focused-window`, niri clamps the tablet position to the visible part of the focused window minus one pixel, and `f64::clamp()` panics when that part is narrower or shorter than one pixel.
    nori clamps to 0 in that case, and maps the tablet as if no window were focused when that part has a width or height of 0 (dividing by it would give a NaN position); the clamping has a unit test in `src/input/mod.rs` ([#205](https://github.com/Deliveos/nori/pull/205); fixes [#151](https://github.com/Deliveos/nori/issues/151)).
- **`stop-cast` without the ScreenCast D-Bus service.**
    `msg action stop-cast --session-id N` crashes a niri instance that runs without the ScreenCast D-Bus interface (for example one started without `--session`), even when no cast matches.
    nori returns early in that case ([e09676aa](https://github.com/Deliveos/nori/commit/e09676aa4273f2163a49fe81101551c01031bad6); fixes [#13](https://github.com/Deliveos/nori/issues/13)).
- **Unexpected vblank events.**
    niri logs a vblank that arrives in an unexpected redraw state ("can happen when resuming from sleep or powering on monitors"), but if an estimated-vblank timer was pending at that moment, it stays registered, and when it, or a timer registered after it, fires in the wrong state, it hits an `unreachable!()` and takes the session down.
    nori removes that timer, and the timer logs an error and keeps the current state instead of panicking if it ever fires in a state it doesn't expect.
    There is no test, because the tests run on the headless backend and this is TTY code ([#201](https://github.com/Deliveos/nori/pull/201); fixes [#147](https://github.com/Deliveos/nori/issues/147)).
- **Empty hotkey overlay.**
    With `hotkey-overlay { hide-not-bound }` and none of the listed actions bound, niri crashes at startup, because the overlay is shown at startup by default.
    nori draws nothing in that case ([0d82a4c5](https://github.com/Deliveos/nori/commit/0d82a4c560a8a4db81193165309f3ad21d558283); fixes [#12](https://github.com/Deliveos/nori/issues/12)).
- **Broken Xcursor files.**
    niri panics when a file of the cursor theme has a valid header but no images, an image size from 2^31 up to 2^31 plus the requested cursor size (larger sizes wrap around and are compared wrongly), or frame delays that add up to more than 2^32 − 1 milliseconds; the release profile checks for overflow too.
    nori treats a file without images like a missing cursor (it tries the other names of the cursor and then falls back to the default or the built-in cursor), compares sizes in 64-bit arithmetic and caps the total delay at 2^32 − 1 ms, with unit tests for each case in `src/cursor.rs` ([#202](https://github.com/Deliveos/nori/pull/202); fixes [#150](https://github.com/Deliveos/nori/issues/150)).
    A short file that declares a large image (up to 32767 × 32767) also makes niri's Xcursor parser allocate the declared pixels, up to 4 × 32767 × 32767 bytes (about 4 GiB), before the read fails; if the kernel refuses that allocation, the process aborts.
    nori first checks that every image's pixels fit in the file, and treats a file that fails the check like a broken one, with a unit test in `src/cursor.rs` ([#249](https://github.com/Deliveos/nori/pull/249); fixes [#237](https://github.com/Deliveos/nori/issues/237)).
    The parser also reads an image again, with two copies of its pixels, for every entry of the table of contents that points at it, so a 2 MiB file with 87,377 entries pointing at one 512 × 512 image makes it allocate 87,377 × 2 × 4 × 512 × 512 bytes (about 170.7 GiB).
    nori also checks that the images of all entries together fit in the file, with a unit test in `src/cursor.rs` ([#260](https://github.com/Deliveos/nori/pull/260); fixes [#259](https://github.com/Deliveos/nori/issues/259)).
- <a id="ipc-request-limit"></a>**IPC request size limit.**
    niri reads an IPC request until a newline with no limit, so a client that never sends one (for example a script piping a large file into the socket) makes it buffer until the out-of-memory killer acts.
    nori reads at most 8 MiB (8388608 bytes) per request line.
    A longer request gets the reply `{"Err":"request is longer than 8388608 bytes"}`, the connection is closed, and the warning is logged.
    The limit was 1 MiB at first and was raised to 8 MiB because spawn requests with a long argument list (about 22000 paths) went over 1 MiB.
    `nori msg` shows that reply instead of "Broken pipe (os error 32)" since [#93](https://github.com/Deliveos/nori/pull/93), which reads the reply even when the rest of its write fails ([#64](https://github.com/Deliveos/nori/pull/64), [#73](https://github.com/Deliveos/nori/pull/73), [#93](https://github.com/Deliveos/nori/pull/93); fixes [#49](https://github.com/Deliveos/nori/issues/49), [#69](https://github.com/Deliveos/nori/issues/69), [#74](https://github.com/Deliveos/nori/issues/74)).
- **Screenshot UI when an output fails to render.**
    niri leaves out outputs whose render fails when it opens the screenshot UI, but still puts the initial selection on the output under the cursor (or the focused one), so capturing, moving or resizing the selection then panics if that output was among the failed ones.
    nori puts the selection on an output that has a screenshot, and has a test for it in `src/tests/screenshot.rs` ([#206](https://github.com/Deliveos/nori/pull/206); fixes [#187](https://github.com/Deliveos/nori/issues/187)).
- <a id="displayconfig-scale"></a>**NaN scale through DisplayConfig.**
    In niri, `org.gnome.Mutter.DisplayConfig.ApplyMonitorsConfig` puts the requested scale into the output config unchecked; a NaN scale ends in a division by zero, so any process on the session bus can crash the compositor.
    nori rejects scales that are NaN, infinite, zero or negative with the D-Bus error "Invalid scale …" before applying anything (niri clamps the non-NaN ones to the 0.1–10 range) ([#96](https://github.com/Deliveos/nori/pull/96); fixes [#95](https://github.com/Deliveos/nori/issues/95)).
- **Screencast max framerate of 0/0.**
    niri divides by the numerator of the max framerate that a screencast consumer negotiates, inside a PipeWire callback, where a panic aborts the process.
    SPA compares fractions by cross-multiplying, so 0/0 counts as inside the offered range; whether PipeWire passes such a value on to niri was not tested.
    nori treats a zero numerator as no framerate limit, and has a unit test for it in `src/screencasting/pw_utils.rs` ([#203](https://github.com/Deliveos/nori/pull/203); fixes [#186](https://github.com/Deliveos/nori/issues/186)).
- **Huge `xdg_positioner` values.**
    The smithay version that niri uses does plain `i32` arithmetic on the positioner values a client sends, and release builds have overflow checks on, so one popup request such as `set_offset(i32::MAX, 0)` takes the session down.
    nori builds against a smithay fork in which this math saturates (see [Building and packaging](#building-and-packaging)) and has a regression test for it ([#101](https://github.com/Deliveos/nori/pull/101); fixes [#90](https://github.com/Deliveos/nori/issues/90)).
- **DRM lease objects after their GPU is removed.**
    When a DRM device is removed (an eGPU or a USB display adapter unplugged), niri drops its DRM lease state and only disables its lease global.
    Destroying that device's lease objects later, submitting a lease request made before the removal, or binding the disabled global (any client can still bind it by its name, since wayland-backend doesn't check whether a global is disabled when a client binds it) makes smithay look the state up by the device's node, which panics in niri.
    Any GPU with DRM leasing is affected; destroying connector or lease objects needs a non-desktop connector, such as a VR headset's.
    nori keeps the lease state of a removed device (with its connectors withdrawn) until a new device with the same node gets one, and rejects lease requests for a removed device.
    After that, the old device's lease objects find the new device's state, so destroying an old lease object can end a new lease with the same lessee id.
    There is no test, because the tests run on the headless backend and this is TTY code ([#208](https://github.com/Deliveos/nori/pull/208); fixes [#148](https://github.com/Deliveos/nori/issues/148)).
- **Negative `xdg_positioner` parent size.**
    In the same smithay version, a negative `set_parent_size` builds a size that fails a debug assertion, so any client can panic builds with debug assertions (the dev profile and `cargo test`); release builds were not affected.
    The smithay fork clamps negative parent sizes to 0 ([#122](https://github.com/Deliveos/nori/pull/122); fixes [#104](https://github.com/Deliveos/nori/issues/104)).
- **Negative sizes in other client requests.**
    In the same smithay version, `wl_surface.damage` and `zwp_text_input_v3.set_cursor_rectangle` with a negative width or height, and `zwlr_layer_surface_v1.set_size` with a width or height above `i32::MAX` (which wraps to a negative value), also build a size that fails a debug assertion.
    So clients can panic builds with debug assertions: any client through `wl_surface.damage`, clients that aren't sandboxed through `set_size` (niri doesn't offer layer-shell to sandboxed ones), and the focused client through `set_cursor_rectangle` while an input method runs.
    The smithay fork ignores negative surface damage, as smithay already does for `wl_surface.damage_buffer`, clamps negative cursor rectangle sizes to 0, and clamps layer sizes above `i32::MAX` to `i32::MAX`.
    The tests are `negative_surface_damage`, `negative_text_input_cursor_rectangle` and `huge_layer_surface_size` in `src/tests/crash_regressions.rs` ([#266](https://github.com/Deliveos/nori/pull/266); fixes [#128](https://github.com/Deliveos/nori/issues/128)).
- **A second toplevel or layer surface for one `wl_surface`.**
    smithay accepts a second `xdg_toplevel` for a surface that already has one (`get_toplevel` twice, or through a second `xdg_surface`), and a second layer surface the same way.
    When the first one isn't mapped yet, niri then hits an `assert!`, so any client (sandboxed ones through xdg-shell) can take the session down with two requests; when it is mapped, niri tracks a second window or layer surface for the same `wl_surface`.
    nori disconnects such a client with a protocol error (`already_constructed` on the `xdg_surface`, or `invalid_surface_state` on the new layer surface), and has four regression tests for it in `src/tests/crash_regressions.rs` ([#197](https://github.com/Deliveos/nori/pull/197); fixes [#144](https://github.com/Deliveos/nori/issues/144)).
- **Errors accepting connections.**
    In niri, an `accept()` error other than "would block" on the IPC socket, the Wayland socket or a security-context socket ends the event loop, and so the session, for example when it runs out of file descriptors (`EMFILE`).
    A security context whose socket can't be polled (a regular file) also panics.
    nori retries right away after `EINTR` and `ECONNABORTED`; after other errors it logs a warning and stops accepting on that socket for 1 second, except on a security-context socket, which comes from a client: nori closes that one, so its client has to create a new security context.
    A security context whose socket can't be polled is dropped with a warning.
    The tests are `security_context_with_unusable_listen_fd` in `src/tests/crash_regressions.rs`, and `accept_error_actions` and `source_does_not_keep_the_event_loop_alive` in `src/utils/listening_socket.rs` ([#209](https://github.com/Deliveos/nori/pull/209); fixes [#149](https://github.com/Deliveos/nori/issues/149)).
- **NUL bytes in hotkey overlay titles.**
    A bind's `hotkey-overlay-title`, and the command of a `spawn` or `spawn-sh` bind that the hotkey overlay lists, can contain a NUL byte (`"\u{0}"`), and niri passes these titles to pango when it shows the overlay or reads it to a screen reader.
    glib's string conversion for pango panics on a NUL byte in builds with debug assertions (the dev profile and `cargo test`); release builds don't make that check.
    nori removes NUL bytes from these titles, with the test `nul_bytes_are_removed_from_titles` in `src/ui/hotkey_overlay.rs` ([#251](https://github.com/Deliveos/nori/pull/251); fixes [#238](https://github.com/Deliveos/nori/issues/238)).
- **Running out of file descriptors.**
    niri also panics, and so ends the session, when creating or copying a file descriptor fails on the main thread in a few other places, for example after it runs out of them (`EMFILE`): the ping that removes a transaction's deadline timer (on commits of windows that resize in one transaction, and when such a window closes), copying the X11 sockets for xwayland-satellite (at startup, when an X11 client connects, and each time xwayland-satellite exits), and the calloop channels created when a config reload starts the xwayland-satellite integration and for each screenshot of a screen, of a window or from the screenshot UI.
    nori logs a warning instead.
    A transaction without that ping keeps its timer until the deadline (300 ms), the X11 socket watch is set up again after 1 second, keeping a connection that is waiting for xwayland-satellite, an integration that fails to start stays off until a config reload changes the `xwayland-satellite` section, as after other errors there, and such a screenshot is neither saved nor copied.
    The channel that nori creates on the main thread when a reload turns its power key handling on (for the inhibitor thread from the `disable-power-key-handling` fix below) had the same problem.
    The tests are `deadline_timer_without_file_descriptors` in `src/utils/transaction.rs`, which lowers the open file limit in a child process, and `channel_handles_messages_sent_before_the_sender_is_dropped` in `src/utils/mod.rs` for the channel that replaces the calloop ones; the other cases aren't tested ([#264](https://github.com/Deliveos/nori/pull/264); fixes [#257](https://github.com/Deliveos/nori/issues/257)).

Most crash fixes from the older work (output positions, workspaces, the window switcher click, clipboard writer threads) come with client-server regression tests in `src/tests/crash_regressions.rs`, which niri doesn't have ([4a000d50](https://github.com/Deliveos/nori/commit/4a000d504c9ab0fd8b7c731a004a90a68b353dd7)); the spring fix has unit tests, the hotkey overlay fix has only a unit test of its precondition (it doesn't call `render()`), and the `stop-cast` and custom mode fixes have none.
Of the later crash fixes above, the IPC request limit and both `xdg_positioner` fixes have cases in that file, and the DisplayConfig scale fix has a unit test in `src/dbus/mutter_display_config.rs`.
Entries added after these say in the entry itself where their test is, or that there is none.

## Freeze and hang fixes

The gamma, blur and suspend fixes follow rule 2 in [AGENTS.md](AGENTS.md#3-golden-rules): don't block the main thread.
The xwayland-satellite fix bounds a wait on the main thread to 2 seconds.
The spawn fix is about a background thread that never finished.

- **`load-config-file` over IPC with a path on a stalled mount.**
    niri checks that the path given to `nori msg action load-config-file --path` is a file with a `stat` on the main thread, so a path on a stalled network or FUSE mount freezes the compositor until `stat` returns.
    nori makes this check on a thread; the reply is the same as before ([#219](https://github.com/Deliveos/nori/pull/219); fixes [#159](https://github.com/Deliveos/nori/issues/159)).
- **Spawning a program whose exec fails.**
    With the systemd feature (on by default), niri's spawner thread hangs forever when the exec of a spawned program fails, for example because the program is missing: each failed spawn leaves a thread and a process behind, and the error is never logged.
    In nori the intermediate child waits on its pipe with a 2-second timeout, so the spawn returns the error after about 2 seconds and nori logs it ([9029b62d](https://github.com/Deliveos/nori/commit/9029b62d31c2d32304e70ece335ddf03a9f7fb3d); fixes [#17](https://github.com/Deliveos/nori/issues/17)).
    A worry that this timeout could leave spawned programs in nori's own cgroup ([#80](https://github.com/Deliveos/nori/issues/80)) was checked and closed as not planned.
- <a id="xwayland-satellite-test"></a>**xwayland-satellite that doesn't exit on the on-demand test.**
    At startup, and on a config reload that changes the `xwayland-satellite` section while the integration isn't running (it was `off`, or an earlier test or socket setup failed), niri runs the configured program with `--test-listenfd-support` on the main thread and waits for it to exit with no time limit, so a `path` to a program that doesn't exit on these arguments hangs startup or freezes the compositor on such a reload.
    nori kills the program after 2 seconds and disables the integration, as when the test fails; startup or such a reload still waits for those 2 seconds.
    The timeout has a unit test in `src/utils/xwayland/satellite.rs` ([#220](https://github.com/Deliveos/nori/pull/220); fixes [#158](https://github.com/Deliveos/nori/issues/158)).
- **Gamma ramps from a pipe.**
    For wlr-gamma-control `set_gamma`, niri reads the ramps from the client's file descriptor on the main thread, so a client that passes a pipe and never writes to it freezes the compositor, including input and VT switching.
    nori reads with `pread`, which fails on pipes instead of blocking, so that request fails and nori logs "failed to read gamma data" ([f40ced8b](https://github.com/Deliveos/nori/commit/f40ced8b4e2cb176042d94867049b7d25239867a); fixes [#8](https://github.com/Deliveos/nori/issues/8)).
- <a id="gamma-reads"></a>**Gamma ramps from a slow file.**
    In niri, and in nori before this fix, the read runs on the main thread with no time limit; `pread` fails on pipes, but waits on a regular file on a FUSE or stalled network file system, so a client that passes such a file freezes the compositor.
    nori reads each ramp on a thread, which also closes the file, and applies the ramp on the main thread when the read finishes.
    A gamma control has at most one read in progress: a `set_gamma` that comes in meanwhile is read after it instead of any earlier one that waits, and replaces the finished read's ramp.
    At most 16 reads can be in progress at once, counting reads of gamma controls that were destroyed meanwhile; a `set_gamma` beyond that fails its gamma control, like a failed read.
    Unit tests in `src/protocols/gamma_control.rs` cover reading the ramp; reading on a thread has no test ([#252](https://github.com/Deliveos/nori/pull/252); fixes [#156](https://github.com/Deliveos/nori/issues/156)).
- **Stopping a screencast.**
    When a cast ends from the compositor's side (its window or output goes away, PipeWire fails, or the user stops it), niri runs the D-Bus `Session.Stop` code with `block_on` on the main thread, which waits for the session's interface lock (held by a running `RecordMonitor` or `RecordWindow` call on that session) and for messages to the bus, and panics if sending the `Closed` signal or removing the D-Bus objects fails.
    nori runs it on a thread and logs these errors instead ([#221](https://github.com/Deliveos/nori/pull/221); fixes [#161](https://github.com/Deliveos/nori/issues/161)).
- **A screencast session that stops during `RecordMonitor` or `RecordWindow`.**
    niri's D-Bus `RecordMonitor` and `RecordWindow` hold the session's interface lock for writing while they wait for the D-Bus object server's lock to add the stream.
    Stopping the session from the compositor's side, and D-Bus `Introspect`, `Properties.Get` and `Properties.GetAll` calls, hold the object server's lock while they wait for the session's interface lock.
    When one of them takes the object server's lock after `RecordMonitor` or `RecordWindow` took the session's lock and before it adds the stream, both sides wait forever: in niri, which stops the session on the main thread, this can freeze the compositor, and in nori since [#221](https://github.com/Deliveos/nori/pull/221) it hung the ScreenCast D-Bus service.
    nori's `RecordMonitor` and `RecordWindow` take the session's interface lock only for reading; a stream that one of them adds after the session stopped is removed again and the call fails.
    A test in `src/dbus/mutter_screen_cast.rs` stops a session while `RecordMonitor` runs ([#250](https://github.com/Deliveos/nori/pull/250); fixes [#244](https://github.com/Deliveos/nori/issues/244)).
- <a id="blur-region-cap"></a>**Huge blur regions.**
    niri converts an ext-background-effect blur region into non-overlapping rectangles on the render path, with no limit on the number of rectangles, and any client can use that protocol.
    In the issue's measurement, 4000 rectangles (about 100 KB of requests) took 9.8 seconds and produced 16 million rectangles.
    For regions with more than 64 rectangles (added and subtracted together), nori blurs the bounding box of the added rectangles instead, so holes in such a region are blurred too ([9a986ef6](https://github.com/Deliveos/nori/commit/9a986ef6e37cf5c325bd262edb64c68e82a38ea1); fixes [#7](https://github.com/Deliveos/nori/issues/7)).
- **The suspend action.**
    niri calls logind's `Suspend` on the main thread over a new system bus connection and waits for the reply, so a slow system bus stalls the compositor, and with `interactive = true` logind can wait for polkit, whose authentication agent is a client of the stalled compositor.
    nori makes the call on a thread, and ignores further suspend actions while one call is still running ([#217](https://github.com/Deliveos/nori/pull/217); fixes [#185](https://github.com/Deliveos/nori/issues/185)).

## Security and the lock screen

- **Exit dialog at the lock screen.**
    In niri, an exit confirmation dialog that is open when the session locks keeps keyboard focus, so if the next key pressed at the lock screen is Enter, niri quits and the session ends.
    nori closes the dialog when locking starts ([85ecb150](https://github.com/Deliveos/nori/commit/85ecb150d83fa2a105656ef287c55d575ebb550e); fixes [#5](https://github.com/Deliveos/nori/issues/5)).
- **Locking while nori's VT is inactive.**
    In niri, a lock request made while its VT is not active (for example a before-sleep locker while the user is on another VT) fails, because nothing is rendered, and on return the desktop is shown unlocked.
    nori keeps the session in the locking state and finishes locking after switching back to its VT ([ae2d2351](https://github.com/Deliveos/nori/commit/ae2d235147b37fb5fc704f343dcdc41a5a1e72ea); fixes [#6](https://github.com/Deliveos/nori/issues/6)).
- **Window screencasts while locked.**
    niri keeps rendering window screencasts while the session is locked, so they show the window behind the lock screen; monitor casts show the lock screen.
    nori doesn't render window casts while the session is locked: not on output redraws ([5b90ed14](https://github.com/Deliveos/nori/commit/5b90ed146fd473ee84c5eb1c36af9412150cbc6c); fixes [#19](https://github.com/Deliveos/nori/issues/19)), and not on redraws that PipeWire asks for or on retargeting a dynamic cast to a window ([#137](https://github.com/Deliveos/nori/pull/137); fixes [#134](https://github.com/Deliveos/nori/issues/134)).
- **Screen transition at the lock screen.**
    niri draws a running screen transition (from `do-screen-transition`, a snapshot of the outputs taken when it ran) on top of the lock screen, and keeps it when the session locks.
    If the session locks during the transition delay (250 ms by default, up to 65535 ms with `--delay-ms`), the frames that confirm the lock to the lock screen client still show the unlocked desktop.
    nori drops all screen transitions when locking starts ([#215](https://github.com/Deliveos/nori/pull/215); fixes [#152](https://github.com/Deliveos/nori/issues/152)).
- <a id="keyboardmonitor"></a>**KeyboardMonitor D-Bus interface.**
    niri serves `WatchKeyboard`, `GrabKeyboard` and `SetKeyGrabs` of `org.freedesktop.a11y.KeyboardMonitor` to any caller on the session bus, so any unsandboxed program can receive every keystroke, including lock screen passwords.
    nori serves them only to the owner of the bus name `org.gnome.Orca.KeyboardMonitor` and returns `AccessDenied` to everyone else; Orca keeps working.
    This stops programs that don't own the name, but a program can still claim the name while Orca isn't running, so [Security-Model.md](docs/wiki/Security-Model.md) still applies ([84183bb6](https://github.com/Deliveos/nori/commit/84183bb6c901603c7320b4a85740f8cc53faf8bb); fixes [#20](https://github.com/Deliveos/nori/issues/20)).
- **Fixed-name D-Bus screenshot in `/tmp`.**
    With `screenshot-path null`, niri writes D-Bus screenshots to `/tmp/screenshot.png`, a fixed name in a directory that all users share.
    nori writes them to `$XDG_RUNTIME_DIR/screenshot.png` and uses the temporary directory only if no runtime directory is set ([dc91dc78](https://github.com/Deliveos/nori/commit/dc91dc785f9c440cbb9f0365a7b38972d0ffcd28); fixes [#14](https://github.com/Deliveos/nori/issues/14)).

## Other bug fixes

### Outputs

- **Monitors whose EDID arrives late.**
    Some devices, notably USB-C docks with DP-MST or alt-mode, report a connector as connected before its EDID was read, and as changed later; niri names the output when the connector is connected and ignores the change, so the output has no make, model and serial, and `output` config sections and IPC requests that use them don't match it.
    nori takes the new name on the change while there is no output for the connector yet and the new name has a make, model or serial that differs from the known name, with the same check for duplicate make/model/serial as for new connectors.
    A change whose EDID read failed keeps the known make, model and serial, so an `off` section that uses them keeps matching.
    The connector gets a new id with the new name, so wlr-output-management clients get a new head with the new make, model and serial.
    An output created before the EDID was read (from fallback modes, a custom mode or a modeline) still keeps its name without make, model and serial ([#232](https://github.com/Deliveos/nori/pull/232); fixes [#167](https://github.com/Deliveos/nori/issues/167)).
- **wlr-output-management keeps config for other outputs.**
    niri replaces the whole outputs config with the heads in a wlr-output-management request, which only covers connected outputs, so settings for unplugged monitors are lost until the output sections of the config file change (a reload with unchanged output sections keeps the applied config).
    nori replaces only the entries with the same name, in place, and keeps the order of the other entries, so a change from wlr-randr, kanshi and similar tools also takes effect when several config entries match one output ([32a109dc](https://github.com/Deliveos/nori/commit/32a109dc0c3a02c5e4620a2f8b98f9cde3f8b187), [#86](https://github.com/Deliveos/nori/pull/86); fixes [#21](https://github.com/Deliveos/nori/issues/21), [#81](https://github.com/Deliveos/nori/issues/81)).
- **DisplayConfig keeps the output entry.**
    In niri, `ApplyMonitorsConfig` (used by GNOME-style display settings) replaces the matched output entry with a new one named after the connector and with default values, so the entry loses settings such as VRR, focus-at-startup and max-bpc, and is renamed, until the output sections of the config file change.
    nori changes only the fields that DisplayConfig sets (on/off, scale, transform, position, mode; a new mode replaces a modeline) ([#94](https://github.com/Deliveos/nori/pull/94); fixes [#89](https://github.com/Deliveos/nori/issues/89)).
- **Custom modes survive an apply.**
    Clients list the current mode and send it back when they change only the scale or position; niri then stores it as a normal mode and drops a custom mode or modeline, switching the output to its preferred mode.
    nori keeps the configured mode or modeline when the requested mode is the current one, for both DisplayConfig and wlr-output-management ([#123](https://github.com/Deliveos/nori/pull/123); fixes [#99](https://github.com/Deliveos/nori/issues/99)).
- **Panel orientation applied twice.**
    niri reports the transform with the DRM panel orientation applied, and stores the transform a client sends back as the configured one, so the orientation is added a second time and the screen turns.
    nori stores the requested transform minus the panel orientation ([#123](https://github.com/Deliveos/nori/pull/123); fixes [#100](https://github.com/Deliveos/nori/issues/100)).
- **Current mode after the mode list changes.**
    Switching between a normal mode and a custom mode or modeline adds or removes a mode, and niri then doesn't tell wlr-output-management clients the new current mode, so a client that was already running shows another mode as current, and sending it back switches to that mode; turning on an output with a custom mode isn't reported to them at all.
    nori sends the current mode, and that the output is on, whenever they change ([#139](https://github.com/Deliveos/nori/pull/139); fixes [#131](https://github.com/Deliveos/nori/issues/131)).
- **wlr-output-management bound twice by one client.**
    niri keeps one wlr-output-management state per client, so when one client binds the manager twice (for example two modules of one panel), the second binding replaces the first: the first manager stops getting changes, `set_mode` with a mode of the first manager is a protocol error that disconnects the client, and releasing a head or stopping either manager acts on the second one.
    nori keeps the state per manager ([#223](https://github.com/Deliveos/nori/pull/223); fixes [#169](https://github.com/Deliveos/nori/issues/169)).
- **Preferred mode after the mode list changes.**
    When the mode list changes, niri rewrites the wlr-output-management clients' existing mode objects in place, by position, and the protocol can't take the `preferred` flag off a mode object, so after adding or removing a custom mode a running client sees another mode as preferred.
    nori replaces all mode objects of the head when its mode list changes, with a test in `src/tests/output_management.rs` ([#245](https://github.com/Deliveos/nori/pull/245); fixes [#195](https://github.com/Deliveos/nori/issues/195)).

### Screenshots

- <a id="screenshots"></a>**D-Bus screenshots reply on failure.**
    In niri, an `org.gnome.Shell.Screenshot` call whose save fails in the background thread never gets a reply, and since all calls share one reply channel, the replies to later calls go to the wrong calls; niri also doesn't create the screenshot directory on this path.
    nori gives each call its own reply, returns a D-Bus error on failure, and creates the `screenshot-path` directory ([dc91dc78](https://github.com/Deliveos/nori/commit/dc91dc785f9c440cbb9f0365a7b38972d0ffcd28); fixes [#14](https://github.com/Deliveos/nori/issues/14)).
- **No overwrites within the same second.**
    niri's default screenshot names have one-second precision, so two screenshots in the same second overwrite each other.
    For names generated from `screenshot-path`, nori adds a numbered suffix such as ` (2)` when the name is taken, up to ` (999)`; an explicit `path` given to a screenshot action is still overwritten ([dc91dc78](https://github.com/Deliveos/nori/commit/dc91dc785f9c440cbb9f0365a7b38972d0ffcd28); fixes [#14](https://github.com/Deliveos/nori/issues/14)).
- **Atomic replace of the fixed-name screenshot.**
    With `screenshot-path null`, every D-Bus screenshot goes to one file, and niri truncates and rewrites it while an earlier caller may still be reading it.
    nori writes to a temporary file next to it and renames it into place when complete ([#85](https://github.com/Deliveos/nori/pull/85); fixes [#83](https://github.com/Deliveos/nori/issues/83)).
- **Cursor after the screenshot UI fails to open.**
    niri sets the crosshair cursor after trying to open the screenshot UI even when it didn't open (for example when every output's screenshot failed to render), so the cursor stays a crosshair until something else sets it.
    nori sets it only when the UI opens. There is no test, because the test fixture can't make every screenshot fail ([#248](https://github.com/Deliveos/nori/pull/248); fixes [#239](https://github.com/Deliveos/nori/issues/239)).
- **Capture button on another monitor.**
    In niri's screenshot UI, pressing the capture button on a monitor other than the one with the selection and moving the pointer before releasing takes no screenshot, because the motion is measured on the selection's monitor and the release then checks the wrong point against the button.
    With monitors placed far apart, the same mix-up can overflow `i32` in the hit test and panic (release builds have overflow checks on).
    nori measures the motion on the button's monitor while the button is held, and does the hit test in `f64` ([#234](https://github.com/Deliveos/nori/pull/234); fixes [#188](https://github.com/Deliveos/nori/issues/188)).
    Test: `capture_button_hit_test` in `src/ui/screenshot_ui.rs` covers the overflow; the coordinate fix has no test.
- **Errors at the end of the PNG write.**
    niri ignores errors from the last part of a D-Bus screenshot's PNG write, so on a full disk a truncated PNG is reported as saved.
    nori reports the error, and the call fails ([#88](https://github.com/Deliveos/nori/pull/88); fixes [#84](https://github.com/Deliveos/nori/issues/84)).
- <a id="screenshot-threads"></a>**Bursts of screenshots.**
    niri starts one thread per screenshot to encode and save the PNG, with no limit, and each thread holds a copy of the pixels (about 33 MB for one 4K output) until it finishes, which for screenshot actions and the screenshot UI includes showing the "Screenshot captured" notification, a D-Bus call that waits for the notification daemon's reply with no time limit; each `org.gnome.Shell.Screenshot` call also renders all outputs on the main thread first, and a failure to start a thread panics.
    nori saves at most 4 screenshots at once, counting screenshot actions, the screenshot UI and D-Bus calls together, so a burst of D-Bus calls from any session-bus client can make the user's own screenshot keys fail for a moment: further D-Bus calls and `screenshot-screen` and `screenshot-window` actions fail before rendering, and the screenshot UI logs "too many screenshots are being saved" when it saves.
    Each thread drops its pixels once the PNG is written, the notification call gives up after 5 seconds, and a failure to start the thread is logged.
    `screenshot_threads_are_limited` in `src/tests/screenshot.rs` tests the counter ([#222](https://github.com/Deliveos/nori/pull/222); fixes [#157](https://github.com/Deliveos/nori/issues/157)).

### Windows and popups

- **Per-workspace `default-column-display`.**
    In niri, a new window gets its column display mode from the monitor's first workspace, so `default-column-display` in a named workspace's `layout {}` block works only when that workspace is the monitor's first one, and then applies to new windows on all of the monitor's workspaces.
    nori applies the target workspace's options before creating the column ([a31350cd](https://github.com/Deliveos/nori/commit/a31350cd6c77d3f3789a723a99f210684e4bcb6f); fixes [#22](https://github.com/Deliveos/nori/issues/22)).
- **`move-column-to-workspace` to another monitor with a floating window.**
    When the focused window is floating and the referenced workspace is on another monitor, niri moves the window to that monitor's active workspace instead of the referenced one, and focuses it even with `--focus false`.
    nori moves it to the referenced workspace and follows `--focus`, like it does on the same monitor ([#210](https://github.com/Deliveos/nori/pull/210); fixes [#173](https://github.com/Deliveos/nori/issues/173)).
- **Popups above or left of their window.**
    niri computes the output overlap of popups from the window's top-left corner, so a popup entirely above or left of its window (a tooltip above it, a menu opening upward) never gets `wl_surface.enter` and can be drawn at the wrong scale.
    nori uses an overlap area centered on the window ([#92](https://github.com/Deliveos/nori/pull/92); fixes [#72](https://github.com/Deliveos/nori/issues/72)).
- **Clicks after `pick-window` and `pick-color`.**
    niri ends a pick on the press of the click and keeps the release from smithay, so smithay counts that button as held until it is pressed and released again: after a pick, a click with another button keeps its click grab after it is released, and so do the interactive resize and view-drag grabs, which keep following the pointer.
    nori still picks on the press, but keeps the grab until all buttons are released, so smithay sees the releases and the window doesn't get them.
    If the pick is cancelled while a button is still held (for example with Esc), that release goes to the surface under the pointer, which didn't get the press.
    The tests are `pick_window_click_is_released` and `pick_color_click_is_released` in `src/tests/pick.rs` ([#231](https://github.com/Deliveos/nori/pull/231); fixes [#166](https://github.com/Deliveos/nori/issues/166)).
- **Input method popups that move to another window.**
    When focus moves, niri doesn't send `wl_surface.leave` for the old window's output to the input method popup, so a client can keep drawing it at the old output's scale.
    nori sends leave for every output when the popup is dismissed; the popup then enters the new window's output as before ([#126](https://github.com/Deliveos/nori/pull/126); fixes [#98](https://github.com/Deliveos/nori/issues/98)).
- **Popups of layer-shell surfaces.**
    niri sends `wl_surface.enter` to a popup of a layer-shell surface only when the layer surfaces of that output are arranged (when one of them commits, maps or unmaps, or the output changes), so a new popup (a bar's menu, an input method popup in a launcher) has no output until then; since the change above, an input method popup on a layer surface also lost its output each time text input was enabled again.
    nori enters the popup on the layer surface's output right away ([#142](https://github.com/Deliveos/nori/pull/142); fixes [#132](https://github.com/Deliveos/nori/issues/132)).
- **Custom close shader with blocked-out windows.**
    With a custom `window-close` shader, niri passes the shader the snapshot for the render target, which for a window with `block-out-from` in a screencast is the blocked-out one, but maps it with the size of the normal snapshot; the normal one is larger when the window draws outside its geometry (for example client-side shadows), so there the animation is drawn at the wrong place and size.
    nori maps each snapshot with its own size ([#230](https://github.com/Deliveos/nori/pull/230); fixes [#177](https://github.com/Deliveos/nori/issues/177)).
- **Resize animation flash.**
    niri takes the starting snapshot of a resize animation on the commit that acknowledges the resize, and Firefox has already committed its content at the new size by then, so maximizing or restoring Firefox flashes.
    nori takes the snapshot of a window on screen when the resize is sent to it; off-screen windows, or a scale change before the window answers, fall back to niri's behavior, and with resize animations off nori skips this early snapshot (the snapshot on commit is still taken, as in niri) ([#111](https://github.com/Deliveos/nori/pull/111); fixes [#110](https://github.com/Deliveos/nori/issues/110)).
- **NaN and infinite column widths.**
    niri's `set-column-width` and `set-window-height` binds accept `nan%` and `inf%`, since Rust's float parser does, and a config proportion too large for a float, like `proportion 1e999`, becomes infinity.
    A NaN column width asks the window for a width of 2147483647 px and stays NaN after relative changes like `+10%`; an infinite default column width asks for the same width, because the default column width skips the limit of 10000 on proportions that `set-column-width` applies.
    nori rejects these values when it parses the config or a size change, and applies the limit to the default column width ([#211](https://github.com/Deliveos/nori/pull/211); fixes [#175](https://github.com/Deliveos/nori/issues/175)).
- **Cursor shape after resize and view-drag grabs.**
    In niri, the resize grab resets the cursor shape without queueing a redraw, and the spatial movement grab redraws only the output the gesture ran on, so an output that nothing else redraws (for example with the cursor over empty space on another output) keeps the resize or drag shape until its next redraw.
    nori redraws the outputs that show the cursor when the grab ends ([#65](https://github.com/Deliveos/nori/pull/65); fixes [#45](https://github.com/Deliveos/nori/issues/45)).
- **Window switcher when a hidden window closes.**
    When a window that the window switcher's current scope or app id filter hides closes, niri animates every visible thumbnail as if a thumbnail had been removed, so the whole strip jumps sideways and slides back.
    nori moves nothing in that case, with a unit test in `src/ui/mru/tests.rs` ([#243](https://github.com/Deliveos/nori/pull/243); fixes [#193](https://github.com/Deliveos/nori/issues/193)).
- **Damage of off-screen textures.**
    niri draws a window's opening animation, a window's fade in or out (while it is dragged with the mouse, and in tabbed columns), and the window switcher's closing fade through an off-screen texture, which it keeps when the content gets smaller.
    It then scales the damage of that texture by the texture size over the content size, so after the content shrank a change is reported at a wrong, larger place, and the part that changed may not be repainted.
    nori reports the damage where the content changed, and has a unit test for it in `src/render_helpers/offscreen.rs` ([#236](https://github.com/Deliveos/nori/pull/236); fixes [#191](https://github.com/Deliveos/nori/issues/191)).
- **Focus after `move-workspace-to-index` with `empty-workspace-above-first`.**
    In niri, moving a workspace to the top with this option compares the focused workspace's index after inserting the new empty workspace with an index from before, so focus can jump to a neighbouring workspace, or to the moved one, without animation.
    nori keeps the same workspace focused ([#200](https://github.com/Deliveos/nori/pull/200); fixes [#172](https://github.com/Deliveos/nori/issues/172)).
- **Closing a column after a view gesture.**
    When a new column opens right of the focused one, niri remembers to focus the previous column again if the new one is closed right away, and forgets this when focus moves to another column, except when a view gesture moves it (a touchpad swipe, a mouse or touchscreen view drag, or edge scrolling during drag-and-drop).
    After such a gesture, closing the newly focused column focuses the column left of it and scrolls to the view position remembered from before, instead of focusing the column that moves into its place (or the one left of it, if it was the last), as closing a column normally does.
    nori forgets it after a gesture too, and has a test for it in `src/layout/tests.rs` ([#218](https://github.com/Deliveos/nori/pull/218); fixes [#174](https://github.com/Deliveos/nori/issues/174)).
- **Maximize and fullscreen requests before a window maps.**
    A window that asks to be maximized, or fullscreened without naming an output, after its initial configure but before it maps, gets the size of the output it was configured for in niri, or of its parent's current output if it follows its parent.
    Unmaximize and unfullscreen requests, and the map unless the window opens next to its parent, use the named workspace that the window was configured for instead (its `open-on-workspace`, or the active workspace if that has a name), so when this workspace moves to another output in between (for example when its `open-on-output` monitor is connected), the window is first sized for the wrong output and resized again right after it maps.
    niri's unmaximize and unfullscreen requests also store the output of that named workspace as the window's output, which can change where it maps: a window that follows its parent then opens on the named workspace instead of next to its parent if the parent and the named workspace are on different outputs by then, and a dialog configured for another output opens next to its parent if the named workspace has moved to the parent's output.
    nori picks the target the same way for all four requests, following the map: the parent's output if the window will open next to its parent, then that named workspace, then the configured output and the active monitor; as before, the output of a fullscreen request comes first, while the map prefers the named workspace ([#255](https://github.com/Deliveos/nori/issues/255)).
    nori keeps the configured output, except for the output of a fullscreen request and for the active monitor, so that a request before the map doesn't change whether the window opens next to its parent.
    For a window that follows its parent, niri's maximize and fullscreen requests already use the parent's output, and nori keeps that.
    If the named workspace is gone by then, niri's unmaximize and unfullscreen requests send a configure that keeps the maximized or fullscreen state; nori uses the active workspace of the picked monitor instead.
    The tests are in `src/tests/window_opening.rs`: `maximize_and_fullscreen_after_initial_configure_use_target_workspace` (a workspace that moves to another output), `maximize_before_map_keeps_a_dialog_on_its_workspace`, `unmaximize_before_map_keeps_a_dialog_on_its_workspace`, `unmaximize_before_map_keeps_a_dialog_next_to_its_parent`, and `maximize_before_map_keeps_a_dialog_next_to_its_parent` for what niri already does; the removed-workspace case has no test ([#254](https://github.com/Deliveos/nori/pull/254); fixes [#24](https://github.com/Deliveos/nori/issues/24)).

### Screencasts

- **Frames skipped for lack of a PipeWire buffer.**
    When a consumer holds every buffer, niri drops the frame but counts its changes as sent, so if the screen then goes still, viewers keep an old picture; it also logs a warning for every skipped frame.
    nori counts a skipped frame as not sent, so the next frame is complete, retries it (first after one frame interval, at least 10 ms, then backing off up to 1 second), and logs the warning at most once every 10 seconds per cast with the number of skipped frames ([9c5fbfb0](https://github.com/Deliveos/nori/commit/9c5fbfb0b637b45bd45c92e113e7d2833d2e26c9), [7a49ebfc](https://github.com/Deliveos/nori/commit/7a49ebfc659f599948cd0a124a1d6d092d36bb49), [#68](https://github.com/Deliveos/nori/pull/68); fixes [#34](https://github.com/Deliveos/nori/issues/34)).
- **Frame pacing.**
    niri moves frames sent slightly early forward to the ideal time, so the time base ends up ahead of the frame that was sent; a second redraw for the same target then sends a duplicate frame and logs "target frame time is below last".
    A frame late by almost a whole interval could also let a duplicate through, for example on a 60 fps cast of a 59.94 Hz output.
    nori moves only late frames back to the ideal time, and restarts the time base after a frame that is almost an interval late ([#115](https://github.com/Deliveos/nori/pull/115), [#120](https://github.com/Deliveos/nori/pull/120); fixes [#114](https://github.com/Deliveos/nori/issues/114), [#118](https://github.com/Deliveos/nori/issues/118)).
- **ScreenCast `Start` called twice.**
    niri's ScreenCast D-Bus service checks a "started" flag on each stream but never sets it, so every `org.gnome.Mutter.ScreenCast.Session.Start` call starts another PipeWire stream for each stream of the session, with the same stream id.
    nori sets the flag, so a later `Start` call doesn't start a stream again; it still starts streams recorded after the first `Start` ([#207](https://github.com/Deliveos/nori/pull/207); fixes [#170](https://github.com/Deliveos/nori/issues/170)).
- **Render errors.**
    After a cast frame fails to render, its buffer can't be bound, or its pixels can't be copied into a shared-memory buffer later, nori sends the next frame in full and, with the cursor sent as metadata, sends a changed cursor bitmap again.
    Mostly this fixes problems that the buffer-age and SHM capture changes below brought in; resetting the cursor state and the reset after a failed bind are new compared with niri ([#120](https://github.com/Deliveos/nori/pull/120), [#138](https://github.com/Deliveos/nori/pull/138); fixes [#116](https://github.com/Deliveos/nori/issues/116), [#129](https://github.com/Deliveos/nori/issues/129)).
    When a cursor bitmap fails to render, niri sends a transparent one and nothing more until the bitmap changes, so the consumer shows no cursor; nori sends the bitmap again with the next frame ([#138](https://github.com/Deliveos/nori/pull/138); fixes [#130](https://github.com/Deliveos/nori/issues/130)).

### Rendering

- **Shader elements on GPUs without instancing.**
    niri draws some elements with its own drawing code instead of smithay's, once per damage rectangle: for example borders and focus rings with rounded corners or gradients, tab indicators, shadows, the resize animation and custom open and close animations.
    Without instancing support (GLES 2 drivers that lack either of the two instancing extensions), niri draws these rectangles in batches of 10, but each batch draws one rectangle and moves on by one instead of 10, so of n = 10k + r damage rectangles (1 ≤ r ≤ 10) only the first k + r are drawn, and parts of the element are missing until a later frame repaints them.
    nori draws every rectangle, as smithay's own renderer does, and has a test for it in `src/render_helpers/shader_element.rs` ([#233](https://github.com/Deliveos/nori/pull/233); fixes [#189](https://github.com/Deliveos/nori/issues/189)).

### Session

- **Several idle inhibitors on one surface.**
    niri keeps a set of surfaces with idle inhibitors and removes a surface when any of its inhibitors is destroyed, so if a client has two inhibitors on one surface and destroys one of them, the screen can blank or lock while the other one still exists.
    nori counts the inhibitors of each surface and stops inhibiting when the last one is destroyed ([#216](https://github.com/Deliveos/nori/pull/216); fixes [#168](https://github.com/Deliveos/nori/issues/168)).
- <a id="portal-at-session-start"></a>**Portal started before the session.**
    xdg-desktop-portal picks its backends once at startup from `XDG_CURRENT_DESKTOP`.
    If it was restarted between two sessions (after the variable was unset at logout), it uses the GTK backend, which has no screencasting, for the whole next session.
    When started with `--session` on systemd, nori stops a running `xdg-desktop-portal.service` without waiting, and D-Bus activation starts it again with nori's environment ([#113](https://github.com/Deliveos/nori/pull/113); fixes [#112](https://github.com/Deliveos/nori/issues/112)).
- **`disable-power-key-handling` on a live reload.**
    niri takes the logind power-key inhibitor only at startup, but its own power-key action follows the live config, so removing the option on a reload makes niri suspend while logind also acts on the key, by default powering off.
    In session instances, nori takes or drops the inhibitor when this option changes, and takes it on a separate thread, since the D-Bus call blocks ([9004e44c](https://github.com/Deliveos/nori/commit/9004e44cb5860979055984c29b05964ef45574ec), [#141](https://github.com/Deliveos/nori/pull/141); fixes [#23](https://github.com/Deliveos/nori/issues/23), [#135](https://github.com/Deliveos/nori/issues/135)).

### IPC

- **Event stream clients during bursts of events.**
    niri queues at most 64 events for each event stream client (such as a bar or `nori msg event-stream`) and drops a client whose queue is full.
    The clients are served on the compositor's own event loop, so they can't take any events while one refresh produces them, and a refresh with more than 64 events (for example, more than 64 windows changing their title at once) drops every event stream client, however fast it reads.
    nori limits each client's queue to 1 MiB (1048576 bytes) of serialized events instead (the initial state is queued even when it is larger), and serializes each event once for all clients instead of once per client ([#214](https://github.com/Deliveos/nori/pull/214); fixes [#171](https://github.com/Deliveos/nori/issues/171)).

## Performance

Numbers in this section are the issues' measurements or estimates; most come from a benchmark that is kept outside the repository ([#58](https://github.com/Deliveos/nori/issues/58)).

### Window stacking

- **Floating windows with many child windows.**
    When niri raises a floating window on focus, adds a floating window, or a client makes a floating window the parent of another window with `xdg_toplevel.set_parent`, it brings that window's descendants right above it.
    It finds them by comparing every window below it with the window and with each descendant found so far, and then moves each descendant up with a remove and an insert in the tile list, which shifts the rest of the list each time; a floating tile is 13544 bytes (measured with `size_of` on nori's `main`).
    So a client with many floating child windows makes this work grow with the square of their number on the main thread.
    nori finds the descendants in one pass with a set of their parents and moves them up with one stable partition of the window and the tiles down to the last descendant ([#226](https://github.com/Deliveos/nori/pull/226); fixes [#160](https://github.com/Deliveos/nori/issues/160)).

### Rendering and redraws

- **Off-screen columns.**
    niri builds render elements for every column of the workspace.
    nori skips columns that are more than half a screen outside the visible area, taking animations, focus rings, shadows and the overview zoom into account ([#3](https://github.com/Deliveos/nori/pull/3); fixes [#26](https://github.com/Deliveos/nori/issues/26)).
- **Pointer motion and cursor changes.**
    niri redraws every output on each pointer motion and each cursor change.
    nori works out which outputs the cursor and the drag-and-drop icon cover and redraws only the outputs where they appeared, moved or disappeared.
    Motion still redraws every output while the screenshot UI, the window switcher, drag-and-drop or a pointer grab other than a plain click is active, and on a focus-follows-mouse focus change or a hot-corner overview toggle; tablet proximity, tablet cursor image changes, pointer-constraint warps and warps by nori itself (warp-mouse-to-focus, moving the cursor to another monitor) still redraw every output, as in niri ([f0c5a85d](https://github.com/Deliveos/nori/commit/f0c5a85d924113c993dd703112a63a11a0242416), [a8072ea2](https://github.com/Deliveos/nori/commit/a8072ea22415e7c3fcb62a7e65ad955a5f238bb1), [d4a4a362](https://github.com/Deliveos/nori/commit/d4a4a3628e54c64ced32912b095036c3df481d62); fixes [#27](https://github.com/Deliveos/nori/issues/27)).
    Tablet tool motion, tip presses and tip releases work like pointer motion: they redraw every output only while the screenshot UI, the window switcher, drag-and-drop, a pointer grab other than a plain click or a tool grab that nori starts (moving a window, the overview) is active.
    A tip press that activates a window or focuses an output still redraws every output, as in niri.
    There is no test, because the test input backend has no tablet events ([#247](https://github.com/Deliveos/nori/pull/247); fixes [#179](https://github.com/Deliveos/nori/issues/179)).
- **Animated cursors.**
    While the cursor is animated (for example a busy cursor), niri keeps every output redrawing at its refresh rate, about 430 redraws per second on three 144 Hz outputs according to the issue.
    nori keeps redrawing only the outputs that show the cursor ([d2a94151](https://github.com/Deliveos/nori/commit/d2a9415143000b03ef96c187e21deab9cb376b88), [c9750ca1](https://github.com/Deliveos/nori/commit/c9750ca1da156022b46826b155e007c5884c556b); fixes [#28](https://github.com/Deliveos/nori/issues/28)), and only when the cursor's next frame is due, with a timer, instead of on every refresh cycle: with 50 ms frames on a 144 Hz output that is 20 redraws per second instead of 144 (calculation).
    A cursor with several images in which fewer than two have a non-zero delay isn't redrawn at all, since its image never changes.
    The test `animated_cursor_redraws_when_its_frame_changes` in `src/tests/pointer_redraws.rs` checks that the timer redraws the output, and the unit test `time_to_next_frame` in `src/cursor.rs` checks the time to the next frame, including for cursors that never change ([#246](https://github.com/Deliveos/nori/pull/246); fixes [#178](https://github.com/Deliveos/nori/issues/178)).
- **Failed on-demand VRR.**
    When enabling on-demand VRR fails, niri tries again on every frame, logging a warning and refreshing the output list each time.
    nori tries again only when the on-demand request turns off and on again, or when the output configuration is applied again, for example by a config reload that changes the output settings ([9fa12722](https://github.com/Deliveos/nori/commit/9fa1272271853364761d7ba068c514cf9fc63e2f); fixes [#31](https://github.com/Deliveos/nori/issues/31)).
- **Exit dialog texture.**
    niri uploads the exit confirmation dialog's texture again on every frame it is shown.
    nori uploads it once per scale while the dialog is shown ([9ecf467e](https://github.com/Deliveos/nori/commit/9ecf467ead4c952adfe70d1df72085b028e41f88); fixes [#43](https://github.com/Deliveos/nori/issues/43)).
- **Config notification.**
    After the config error notification (or the one about a newly created config) slides in, niri keeps every output redrawing at its refresh rate for the 4 or 8 seconds that it sits still: for one config error on three 144 Hz outputs, 3 × 144 × 4 = 1728 redraws of a still screen.
    nori redraws only while the notification slides in or out, and once when it should start hiding, with a unit test in `src/ui/config_error_notification.rs` ([#241](https://github.com/Deliveos/nori/pull/241); fixes [#192](https://github.com/Deliveos/nori/issues/192)).
- **Shadows turned off.**
    niri computes shadow geometry for every tile on every frame even when shadows are off (the default).
    nori skips it when shadows are off ([0d07ecd1](https://github.com/Deliveos/nori/commit/0d07ecd11d434564bb3c58e98be90879fc46cddd); fixes [#44](https://github.com/Deliveos/nori/issues/44)).
- **Background blur at the output edge.**
    niri sizes the texture for a non-xray background effect (such as blur) by the element's on-screen part, so while the element crosses the output edge it allocates a new texture and a new blur chain on every frame: with the default 3 blur passes about 2.33 × width × height × 4 bytes, about 19 MB per frame for a 1920 × 1080 element, for each render target that draws it (calculation from the issue, not measured).
    On each axis where a texture for the whole element fits in the output size, nori sizes the texture by the whole element, so on that axis its size stays the same while the element crosses the output edge and is at most the output size.
    On the other axes nori sizes it as niri does: by the on-screen part, divided by the zoom when the element is zoomed out (as in the overview), which can be larger than the output.
    nori fills the off-screen part of the texture by stretching the edge rows and columns of the on-screen part with nearest filtering, so it holds the edge pixels that niri's clamp-to-edge sampling reads beyond its texture edge: exactly when the texture isn't scaled, and approximately when it is (as in the overview).
    In exchange, the blur of a partly visible element runs over its whole size on those axes instead of only its on-screen part, which costs at most as much as when the element is fully on screen.
    In the tests, without blur and at scale 1, the drawn pixels are the same as in niri; with blur, or when the texture is scaled as in the overview, the result can differ slightly, because the blur then also runs over the off-screen part and the on-screen part lands on rounded texture pixels.
    Tests are in `src/render_helpers/framebuffer_effect.rs` ([#242](https://github.com/Deliveos/nori/pull/242); fixes [#190](https://github.com/Deliveos/nori/issues/190)).

### Work on every event loop iteration

niri does some bookkeeping on every iteration of its event loop, even when nothing changed.

- **Tiled state.**
    niri recomputes every window's tiled state on every iteration.
    nori updates it when its inputs change: decoration requests, window rule changes, mapping, and config reloads ([80fa2573](https://github.com/Deliveos/nori/commit/80fa25737cd83da302e2664bef83bbb6abcc30c0), [f364ab12](https://github.com/Deliveos/nori/commit/f364ab12da5bb2f8645d7a983bcf049025656a45); fixes [#41](https://github.com/Deliveos/nori/issues/41)).
- **Pointer inactivity timer.**
    With `cursor { hide-after-inactive-ms }`, niri removes and recreates a timer on every iteration with pointer activity.
    nori keeps one timer that reschedules itself ([3b5388f2](https://github.com/Deliveos/nori/commit/3b5388f2fb71029fba5b72650f3ea7de0501cea2); fixes [#40](https://github.com/Deliveos/nori/issues/40)).
- **Screencast output tracking.**
    niri's `refresh_mapped_cast_outputs` builds IPC window layouts it doesn't use and a new set of all windows on every iteration, even with no screencast running.
    nori skips the layouts and marks windows with a generation counter instead ([a37382a1](https://github.com/Deliveos/nori/commit/a37382a1b0dcf78153f55b557899e0f5afe214da), [3c601ac7](https://github.com/Deliveos/nori/commit/3c601ac7a2ced1d07186b6bd1225cc92cab69e69); fixes [#38](https://github.com/Deliveos/nori/issues/38)).
- **Foreign toplevel close detection.**
    niri's foreign-toplevel refresh looks up every tracked toplevel with a linear search, which is O(n²) in the number of windows (8 µs per iteration at 100 windows and 24.5 µs at 200 in the issue's measurement), and builds unused IPC layouts.
    nori marks the windows it sees in one pass, and looks windows up without cloning their surface handle ([81f7c1f3](https://github.com/Deliveos/nori/commit/81f7c1f3829193f40f697bcdaf3207956583cb2a), [2d42ddb0](https://github.com/Deliveos/nori/commit/2d42ddb0a82f8823a68a309f686143187d561ecb), [#66](https://github.com/Deliveos/nori/pull/66); fixes [#37](https://github.com/Deliveos/nori/issues/37), [#54](https://github.com/Deliveos/nori/issues/54)).
    As a side effect, `closed` events now come after the updates of other windows and before the newly focused window's update.
- **IPC window list.**
    niri asks each window for its size four times while refreshing the IPC window list, and builds a new set of all window ids to find closed windows.
    nori asks once, and collects window ids only when a window was closed ([#59](https://github.com/Deliveos/nori/pull/59); fixes [#50](https://github.com/Deliveos/nori/issues/50), [#55](https://github.com/Deliveos/nori/issues/55)).
- **Image-copy-capture constraints.**
    To see whether an ext-image-copy-capture session needs new buffer constraints, niri reads the session's size through smithay's `current_constraints()`, which clones all of the constraints (the SHM and DMA-BUF format lists with their modifiers), once per iteration for each output capture session and on each refresh of each cursor capture session.
    nori keeps the size it last sent next to each session and compares with that ([#235](https://github.com/Deliveos/nori/pull/235); fixes [#181](https://github.com/Deliveos/nori/issues/181)).
- **Dead output surfaces.**
    niri prunes dead surfaces from its outputs on every iteration; nori does it on its existing one-second timer ([#62](https://github.com/Deliveos/nori/pull/62); fixes [#56](https://github.com/Deliveos/nori/issues/56)).
- **Configure state.**
    niri clones and compares every window's configure state on every iteration.
    nori skips setters whose value didn't change and drops a pending state that equals the last one sent ([#70](https://github.com/Deliveos/nori/pull/70); fixes [#52](https://github.com/Deliveos/nori/issues/52)).
- **Window outputs.**
    niri walks every window's surface tree on every iteration to send `wl_surface.enter`.
    nori does it only for windows whose surface tree or popups committed ([#71](https://github.com/Deliveos/nori/pull/71); fixes [#51](https://github.com/Deliveos/nori/issues/51)).
    Input method popups moved to another window without a commit missed `wl_surface.enter` because of this; [#92](https://github.com/Deliveos/nori/pull/92) fixed that ([#75](https://github.com/Deliveos/nori/issues/75)).

### Destroying surfaces

- **Root surface cache.**
    On every `wl_surface` destruction, niri scans its cache of the root of every surface that has committed, so destroying many surfaces at once, for example when a client with many surfaces disconnects, takes time that grows with the square of their number.
    nori keeps an index from each root to its other surfaces, and removes the destroyed surface's entry and those of the surfaces whose root it was without looking at the others ([#227](https://github.com/Deliveos/nori/pull/227); part of [#154](https://github.com/Deliveos/nori/issues/154)).
    smithay still scans its list of all surfaces on each destruction (`retain` in `SurfaceUserData::destroyed`); that part of the issue needs a change in the smithay fork.

### Screencasts and screen capture

- **GetWindows D-Bus call.**
    On each call, niri's `GetWindows` D-Bus handler, used by the screencast window picker, builds IPC layouts for all windows and throws them away; nori doesn't build them ([#63](https://github.com/Deliveos/nori/pull/63); fixes [#57](https://github.com/Deliveos/nori/issues/57)).
- **Cursor-only changes.**
    With the cursor sent as PipeWire metadata, niri renders the whole frame again when only the cursor moved.
    nori sends a buffer that carries only the cursor metadata; this was tested with OBS ([#107](https://github.com/Deliveos/nori/pull/107); fixes [#32](https://github.com/Deliveos/nori/issues/32)).
- **Buffer age.**
    niri renders every DMA-BUF cast frame in full (buffer age 0), even when a few pixels changed.
    nori passes the real buffer age for PipeWire DMA-BUF casts, so only the damaged area is rendered; DMA-BUF frames for screencopy and image-copy-capture clients are still rendered in full.
    It helps only while at most 5 buffers rotate, which the new default of 4 buffers allows ([#108](https://github.com/Deliveos/nori/pull/108); fixes [#29](https://github.com/Deliveos/nori/issues/29)).
- **SHM captures.**
    For screencopy, image-copy-capture and PipeWire casts without DMA-BUF, niri allocates a full-size texture for every frame and waits on the main thread for the GPU readback.
    nori reuses one texture per capture and, when the renderer supports fences, copies the pixels once the GPU is done instead of waiting; the copy itself still runs on the main thread ([#109](https://github.com/Deliveos/nori/pull/109); fixes [#30](https://github.com/Deliveos/nori/issues/30)).
    A wlr-screencopy manager frees its SHM texture (about 33 MB for a 4K output) once no SHM copy has used it for 10 seconds and none of its copies waits for damage, because clients such as xdg-desktop-portal-wlr keep the manager alive between casts; before this fix, the texture was kept for as long as the manager lived ([#229](https://github.com/Deliveos/nori/pull/229); fixes [#163](https://github.com/Deliveos/nori/issues/163)).

## Memory

- **Blur and xray buffers.**
    niri keeps each output's blur/xray effect buffers (an output-sized texture plus a blur chain for each render target) for as long as the output exists; the issue estimates about 465 MB on a 4K output after a blurred window, the overview, a screencast and a screenshot.
    nori frees buffers that no frame has used for 10 seconds and creates them again when needed; buffers that a window's unmap snapshot still references (it is drawn when the window closes) are kept ([76e7a0fe](https://github.com/Deliveos/nori/commit/76e7a0fef9a735f31f8f9d09fd9a22ae01c1db83), [f8350b40](https://github.com/Deliveos/nori/commit/f8350b4019a17ca2deb995ea0d1b19a68c8a75d8); fixes [#25](https://github.com/Deliveos/nori/issues/25)).
- <a id="pipewire-buffers"></a>**PipeWire buffers.**
    niri offers PipeWire consumers 8 buffers by default, each a whole frame, about 265 MB per 4K stream.
    nori offers 4 by default and still allows 2–16 for consumers that ask; the default is not configurable.
    Consumers without a preference of their own, such as OBS, get 4 ([#106](https://github.com/Deliveos/nori/pull/106); fixes [#33](https://github.com/Deliveos/nori/issues/33)).
- <a id="window-cast-size"></a>**Window screencast size.**
    niri sizes a window screencast to the bounding box of the window's surfaces and popups, with no limit, so a client that puts a small subsurface far from its window makes every stream buffer that large: with one 16000 px away on both axes, 16000 × 16000 × 4 B ≈ 1 GB per buffer, and every frame is rendered at that size.
    nori crops the cast area on each axis to at most the logical size of the output the window is on, rounded up to whole logical pixels.
    The area keeps the window geometry and splits the room left between both sides; on an axis where the window geometry itself is larger than the output, the area is the start (left or top part) of the window geometry.
    Anything outside the area is not cast: client-drawn shadows, subsurfaces, popups, and the rest of a window that is larger than its output.
    A window that doesn't set its geometry has the bounding box of its surface and subsurfaces as its geometry, so a far-away subsurface makes that geometry larger than the output, and the cast shows the start of it.
    The tests are `window_cast_area_is_cropped_around_the_window` and `window_cast_area_is_cropped_without_window_geometry` in `src/tests/window_cast.rs`, and `crop_axis_keeps_the_window_and_splits_the_rest` in `src/window/mapped.rs` ([#253](https://github.com/Deliveos/nori/pull/253); fixes [#165](https://github.com/Deliveos/nori/issues/165)).
- **Hotkey overlay textures.**
    niri keeps the hotkey overlay's texture for each output after the overlay is closed, and the overlay is shown at startup by default; the issue estimates about 10 MB per output at scale 2.
    nori frees them when the overlay is hidden ([96708679](https://github.com/Deliveos/nori/commit/96708679522003d8d4eb443169e64cd147617647); fixes [#42](https://github.com/Deliveos/nori/issues/42)).
- **Closing windows.**
    For every closing window, niri also renders its blocked-out contents into a second window-sized texture (33 MB for a fullscreen 4K window), which is only used with a `block-out-from` rule.
    nori does this only for windows with such a rule ([93ffe3bc](https://github.com/Deliveos/nori/commit/93ffe3bc61b0f084caa38da2f17506ecd591b47f), [e9b214ff](https://github.com/Deliveos/nori/commit/e9b214ff5baf6d82e06ae451993e71322a0fefd3); fixes [#36](https://github.com/Deliveos/nori/issues/36)).
- **Screenshot UI and screen transitions.**
    niri renders every output three times, once per render target, when the screenshot UI opens or a screen transition starts.
    Without `block-out-from` rules the three are identical, so nori renders each output once and shares the texture ([#67](https://github.com/Deliveos/nori/pull/67); fixes [#35](https://github.com/Deliveos/nori/issues/35)).
- **Gamma blobs of unplugged outputs.**
    niri creates a DRM property blob for each gamma ramp (`GAMMA_LUT`, or `CTM` as a fallback) and destroys the previous one when it sets a new one, but not the last one when the output is unplugged, so each unplug with a ramp set (for example while a night-light tool runs) leaks one blob in the kernel until niri exits, `gamma_lut_size` × 8 bytes for `GAMMA_LUT`.
    nori destroys it when the output is disconnected ([#228](https://github.com/Deliveos/nori/pull/228); fixes [#162](https://github.com/Deliveos/nori/issues/162)).
- **Layer surfaces of unplugged outputs.**
    niri keeps an entry for each layer surface (bar, wallpaper) of an output that is removed, one more on each unplug.
    nori removes them with the output ([0280b8b2](https://github.com/Deliveos/nori/commit/0280b8b2f20e84788b614db46d6face684804b44); fixes [#18](https://github.com/Deliveos/nori/issues/18)).
- **Cursor pixels.**
    The Xcursor parser returns every frame's pixels in two byte orders, and niri keeps both for every loaded cursor although it reads only one.
    nori frees the unused copy when it loads a cursor, which saves width × height × 4 bytes per frame, for example 576 KiB (48 × 48 × 4 × 64) for a 64-frame animated cursor at size 48 ([#202](https://github.com/Deliveos/nori/pull/202); fixes [#180](https://github.com/Deliveos/nori/issues/180)).
- **ScreenCast D-Bus sessions.**
    niri keeps every ScreenCast D-Bus session it ever created in a list that is never read.
    nori removes that list; the same commit removes other dead code ([63f4474e](https://github.com/Deliveos/nori/commit/63f4474ee4acf4a0bff44844ee3e0cbdadbd3059); part of [#24](https://github.com/Deliveos/nori/issues/24)).

## Building and packaging

- **smithay fork.**
    niri builds smithay and smithay-drm-extras from [Smithay/smithay](https://github.com/Smithay/smithay) at `8a270232`.
    nori builds them from [Deliveos/smithay](https://github.com/Deliveos/smithay) at `e4670d04`, on its `nori` branch: the same version plus the `xdg_positioner` fixes for [#90](https://github.com/Deliveos/nori/issues/90) and [#104](https://github.com/Deliveos/nori/issues/104), and the negative size fixes for [#128](https://github.com/Deliveos/nori/issues/128).
    Builds that fetch or vendor the dependencies, including the Nix flake, get smithay from there, so the pinned fork commits have to stay reachable from a branch or tag of the fork ([#101](https://github.com/Deliveos/nori/pull/101), [#122](https://github.com/Deliveos/nori/pull/122), [#266](https://github.com/Deliveos/nori/pull/266)).
    Switching back to upstream smithay once the fixes are there is tracked in [#105](https://github.com/Deliveos/nori/issues/105).
- The renamed packaging files and Nix flake attributes are listed under [Names](#names).

## Tests and CI

- Client-server regression tests for the crash and leak fixes ([4a000d50](https://github.com/Deliveos/nori/commit/4a000d504c9ab0fd8b7c731a004a90a68b353dd7), later extended), and a fix for one of them that failed when run with other tests ([#103](https://github.com/Deliveos/nori/pull/103); fixes [#102](https://github.com/Deliveos/nori/issues/102)).
- Tests for foreign-toplevel close detection ([#66](https://github.com/Deliveos/nori/pull/66); fixes [#53](https://github.com/Deliveos/nori/issues/53)) and for the pointer inactivity timer ([#87](https://github.com/Deliveos/nori/pull/87); fixes [#82](https://github.com/Deliveos/nori/issues/82)).
- Screenshot tests that need EGL carry the `egl_` prefix, so the Nix sandbox skips them ([#91](https://github.com/Deliveos/nori/pull/91); fixes [#79](https://github.com/Deliveos/nori/issues/79)), and the nix CI job fails when `nix flake check` fails; niri lets that job pass anyway ([#121](https://github.com/Deliveos/nori/pull/121); fixes [#97](https://github.com/Deliveos/nori/issues/97)).
- CI runs once per pull request commit: pushes start CI only on `main`, and a newer commit cancels the older run ([#2](https://github.com/Deliveos/nori/pull/2)); AGENTS.md and CONTRIBUTING.md describe when CI runs here ([#2](https://github.com/Deliveos/nori/pull/2), [#46](https://github.com/Deliveos/nori/pull/46)).

## Project rules

niri doesn't accept code, issues or pull request descriptions written by LLMs.
nori welcomes people and AI agents under the same rules: [AGENTS.md](AGENTS.md), [CONTRIBUTING.md](CONTRIBUTING.md) and the pull request template ([95e89dc0](https://github.com/Deliveos/nori/commit/95e89dc043105db77c0b614b5c4a76645e6ba544)).
Agents never review, approve or merge a pull request with commits they wrote; another agent or a person merges it once CI is green and no review threads are unresolved ([#61](https://github.com/Deliveos/nori/pull/61), which replaced the rule from [bb073b08](https://github.com/Deliveos/nori/commit/bb073b08e21110bf504f395aa334a907ac57ef1a) that let agents merge their own; fixes [#60](https://github.com/Deliveos/nori/issues/60)).
The issue templates, the README and the contributing guides point at this repository ([#4](https://github.com/Deliveos/nori/pull/4), [#48](https://github.com/Deliveos/nori/pull/48), [#78](https://github.com/Deliveos/nori/pull/78); fixes [#47](https://github.com/Deliveos/nori/issues/47), [#76](https://github.com/Deliveos/nori/issues/76)).

## Upstream merges

nori takes upstream niri in with merge pull requests, so upstream's history is kept.
Upstream's own changes are not listed in this file.

| Pull request | Upstream commits | Opened |
|---|---|---|
| [#117](https://github.com/Deliveos/nori/pull/117) | [e19df12](https://github.com/niri-wm/niri/commit/e19df12cf81ffe805609fee497da1a097066ad2f)..[9d8f4f8](https://github.com/niri-wm/niri/commit/9d8f4f89f6a075a04ed24af5f13829d0bdba5b85) (18 commits) | 2026-10-26 |
| [#268](https://github.com/Deliveos/nori/pull/268) | [9d8f4f8](https://github.com/niri-wm/niri/commit/9d8f4f89f6a075a04ed24af5f13829d0bdba5b85)..[a8562a1](https://github.com/niri-wm/niri/commit/a8562a1126922ed4c1c94d34c1dbb26633491d05) (20 commits) | 2026-11-03 |

To see the upstream commits that nori doesn't have yet, run `git fetch https://github.com/niri-wm/niri main` and then `git log --oneline HEAD..FETCH_HEAD`.

## Open problems

Open issues in nori's own changes:

- [#105](https://github.com/Deliveos/nori/issues/105): keep the pinned smithay fork commits reachable, and switch back to upstream smithay.
- [#58](https://github.com/Deliveos/nori/issues/58): keep the performance benchmarks in the repository.

Problems found in upstream niri, or in the smithay version it uses, while working on nori, not fixed yet: [#154](https://github.com/Deliveos/nori/issues/154) (the smithay part), [#269](https://github.com/Deliveos/nori/issues/269) (switching VT waits for libseat on the main thread).
