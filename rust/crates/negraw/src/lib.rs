#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
//! `negraw` — RAW files → [`negkit::RgbImage`] under the LibRaw contract SwiftInvert pins
//! (sensor-native camera RGB, unity white balance, no camera matrix, black-subtracted,
//! white-normalized to [0, 1], EXIF orientation baked). Owner: wave 1 / agent R. See `rust/PLAN.md` §3.6.
