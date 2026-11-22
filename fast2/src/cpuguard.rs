//! CPU guard. This binary is built for Cascade Lake (AVX-512). Before anything else runs, check that
//! the CPU and the OS support every instruction set it may use; if not, hand over to the portable
//! pipeline at /fast with the same arguments and environment. Kept to integer code and CPUID so the
//! check itself runs on any x86-64 CPU.

use core::arch::x86_64::{__cpuid, __cpuid_count};

const ALT: &[u8] = b"/fast\0";

extern "C" {
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
}

#[inline(never)]
fn xcr0() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: only called after CPUID reported OSXSAVE, so XGETBV is available.
    unsafe { core::arch::asm!("xgetbv", in("ecx") 0u32, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
    (hi as u64) << 32 | lo as u64
}

#[inline(never)]
fn supported() -> bool {
    // SAFETY: CPUID exists on every x86-64 CPU.
    let l0 = unsafe { __cpuid(0) };
    if l0.eax < 7 {
        return false;
    }
    let l1 = unsafe { __cpuid(1) };
    // FMA 12, MOVBE 22, POPCNT 23, OSXSAVE 27, AVX 28.
    let need1 = (1 << 12) | (1 << 22) | (1 << 23) | (1 << 27) | (1 << 28);
    if l1.ecx & need1 != need1 {
        return false;
    }
    // OS saves SSE, AVX, opmask, ZMM_Hi256 and Hi16_ZMM state.
    if xcr0() & 0xE6 != 0xE6 {
        return false;
    }
    let l7 = unsafe { __cpuid_count(7, 0) };
    // BMI1 3, AVX2 5, BMI2 8, AVX512F 16, AVX512DQ 17, AVX512CD 28, AVX512BW 30, AVX512VL 31.
    let need7b: u32 = (1 << 3) | (1 << 5) | (1 << 8) | (1 << 16) | (1 << 17) | (1 << 28) | (1 << 30) | (1 << 31);
    // AVX512_VNNI 11.
    let need7c: u32 = 1 << 11;
    if l7.ebx & need7b != need7b || l7.ecx & need7c != need7c {
        return false;
    }
    let e0 = unsafe { __cpuid(0x8000_0000) };
    if e0.eax < 0x8000_0001 {
        return false;
    }
    // LZCNT (ABM) 5.
    let e1 = unsafe { __cpuid(0x8000_0001) };
    e1.ecx & (1 << 5) != 0
}

/// True if the environment holds exactly `XL_PORTABLE=1` (forces the hand-over, for testing).
#[inline(never)]
fn forced(envp: *const *const u8) -> bool {
    if envp.is_null() {
        return false;
    }
    let want = b"XL_PORTABLE=1\0";
    let mut i = 0;
    loop {
        // SAFETY: envp is the NULL-terminated environment array the loader passed in.
        let e = unsafe { *envp.add(i) };
        if e.is_null() {
            return false;
        }
        let mut j = 0;
        loop {
            // SAFETY: e is a NUL-terminated string; we stop at its NUL or at the first mismatch.
            let c = unsafe { *e.add(j) };
            if c != want[j] {
                break;
            }
            if c == 0 {
                return true;
            }
            j += 1;
        }
        i += 1;
    }
}

extern "C" fn guard(_argc: i32, argv: *const *const u8, envp: *const *const u8) {
    if supported() && !forced(envp) {
        return;
    }
    // SAFETY: argv and envp are the loader's arrays; on failure execve returns and we carry on.
    unsafe { execve(ALT.as_ptr(), argv, envp) };
}

#[used]
#[link_section = ".init_array"]
static GUARD: extern "C" fn(i32, *const *const u8, *const *const u8) = guard;
