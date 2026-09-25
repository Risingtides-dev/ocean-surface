//! Coding-plans controls that measurement proves nothing else holds.
//!
//! Same lane and discipline as `unheld_device_controls.rs`: each control was
//! mutated for real and the wasm clippy gate run; only what stayed GREEN is
//! pinned.
//!
//! ## Measured
//!
//! | Control | Result |
//! |---|---|
//! | `app.rs` header overflow's `Coding plans` row | GREEN — pinned |
//! | `app.rs` `<CodingPlansPanel>` mount | RED — compiler-held |
//! | `coding_plans.rs` row `Sign in` click | RED — compiler-held (`sign_in` unused) |
//! | `coding_plans.rs` row `Sign out` ARMING click | RED — compiler-held (`arm_logout` unused) |
//!
//! The overflow row and the ⌘K `coding-plans` command both call
//! `CodingPlansState::show`, so deleting either one leaves `show` referenced
//! by the other and nothing warns. Deleting both is a `never used` error. The
//! overflow row is the one pinned because it is the door on a phone, where
//! there is no ⌘K.

mod common;

use common::{view_source, without_whitespace};

#[test]
fn the_header_overflow_offers_a_way_into_coding_plans() {
    let view = without_whitespace(&view_source("app.rs"));
    assert!(
        view.contains("coding_plans.show();}>\"Codingplans\"</button>"),
        "the header overflow's `Coding plans` row is the phone's only door to \
         the panel; deleting it leaves `show` still called by the palette, so \
         nothing warns",
    );
}
