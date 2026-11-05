// SPDX-License-Identifier: AGPL-3.0-or-later

pub mod build;
pub mod equity;
pub mod progress;
pub mod run;
pub mod spec;

pub const EXIT_OK: u8 = 0;
pub const EXIT_INVALID_SPEC: u8 = 10;
pub const EXIT_CARD_COLLISION: u8 = 12;
pub const EXIT_OOM: u8 = 21;
pub const EXIT_TIMEOUT: u8 = 22;
pub const EXIT_INTERNAL: u8 = 50;

#[inline(never)]
pub fn cpu_runs_this_build() -> bool {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    unsafe {
        use std::arch::x86_64::{__cpuid, __cpuid_count, _xgetbv};
        const LEAF1: u32 = 1 << 12 | 1 << 20 | 1 << 22 | 1 << 23 | 1 << 27 | 1 << 28 | 1 << 29;
        const LEAF7: u32 = 1 << 3 | 1 << 5 | 1 << 8;
        const EXTENDED: u32 = 1 << 5;
        if __cpuid(0).eax < 7 || __cpuid(0x8000_0000).eax < 0x8000_0001 {
            return false;
        }
        if __cpuid(1).ecx & LEAF1 != LEAF1 {
            return false;
        }
        if _xgetbv(0) & 6 != 6 {
            return false;
        }
        __cpuid_count(7, 0).ebx & LEAF7 == LEAF7 && __cpuid(0x8000_0001).ecx & EXTENDED == EXTENDED
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
    true
}

#[derive(Debug)]
pub struct CliError {
    pub code: u8,
    pub message: String,
}

impl CliError {
    pub fn new(code: u8, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}
