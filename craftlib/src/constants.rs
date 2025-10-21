use std::ops::Range;

use pod2::middleware::{EMPTY_VALUE, RawValue};

pub const COPPER_BLUEPRINT: &str = "copper";
pub const COPPER_MINING_RANGE: Range<u64> = 0..0x0020_0000_0000_0000;
pub const COPPER_WORK: RawValue = EMPTY_VALUE;
