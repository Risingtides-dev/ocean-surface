//! Device controls that measurement proves nothing else holds.
//!
//! The device picker is reachable two ways and populated one way, and only
//! some of that is held by the compiler. Same lane and same discipline as
//! `unheld_room_controls.rs`: every control below was mutated for real on this
//! branch and the wasm clippy gate actually run, and only what stayed GREEN is
//! pinned here.
//!
//! ## Measured
//!
//! | Control | Result |
//! |---|---|
//! | `app.rs` boot `devices.load(true)` | GREEN — pinned |
//! | `app.rs` header overflow's `Devices` row | GREEN — pinned |
//! | `app.rs` `<DeviceChip>` mount | RED — compiler-held |
//! | `app.rs` `<DevicePicker>` mount | RED — compiler-held |
//!
//! The two held ones announce themselves loudly, which is why they are
//! recorded rather than pinned: deleting the chip's mount leaves
//! `DeviceChip`'s generated props struct with a never-read `state` field, and
//! deleting the picker's mount takes `daemon_for_devices` unused AND
//! `Daemon::reattach_to_selected_device` — the entire re-attach path — dead
//! with it. Both are `error:` under `-D warnings`.
//!
//! The two silent ones are silent for the shape this lane keeps finding: they
//! are the only *entry* to machinery that stays fully referenced without them.
//! Delete the boot load and `DeviceState::load` is still called by `select`;
//! delete the menu row and `open` is still written by the chip and still read
//! by the picker. Nothing goes unreferenced. What goes is a person's ability
//! to see, or reach, the machines their login owns — the whole slice, silently.
//!
//! Both needles name the CALL SITE and run over `view_source`, per the lane's
//! two rules, and both were verified by a rename as well as a deletion.

mod common;

use common::{view_source, without_whitespace};

/// Nothing else asks the proxy which machines this login has.
///
/// `DeviceState::load` survives the deletion because `select` calls it again
/// after a successful switch — but a switch can only happen from a picker that
/// was populated, so with the boot call gone the list is empty forever, the
/// chip never appears (it renders on a non-empty `selected`), and the menu row
/// never appears either. The whole feature is absent and every gate is green.
#[test]
fn boot_asks_the_proxy_which_machines_this_login_has() {
    let view = without_whitespace(&view_source("app.rs"));
    assert!(
        view.contains("crate::devices::DeviceState::new();devices.load(true);"),
        "app boot is the only thing that populates the device list; without it \
         the picker is empty, the header chip never appears, and a person with \
         two machines is silently pinned to one",
    );
}

/// The header overflow is the rail's way in, and it is not the chip's.
///
/// The chip is compiler-held and this row is not, so they fail differently:
/// losing the chip is a build error, losing this row is nothing at all. It
/// matters on its own because the chip renders inside the status cluster — the
/// part of the header `compact.css` squeezes hardest — while the overflow menu
/// is where this surface has agreed secondary actions live.
#[test]
fn the_header_overflow_offers_a_way_into_the_device_picker() {
    let view = without_whitespace(&view_source("app.rs"));
    assert!(
        view.contains("devices.open.set(true);}>\"Devices\"</button>"),
        "the header overflow's `Devices` row is the rail's only door to the \
         picker; deleting it leaves `open` still written by the chip and still \
         read by the picker, so nothing warns",
    );
}
