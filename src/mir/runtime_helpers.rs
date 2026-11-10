use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI64, Ordering};

static GAS_REMAINING: AtomicI64 = AtomicI64::new(1_000_000_000);
static LAST_STATUS: AtomicI64 = AtomicI64::new(0);
static MAX_MEMORY_LIMIT: AtomicI64 = AtomicI64::new(i64::MAX);
static MEMORY_USED: AtomicI64 = AtomicI64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_set_gas(gas: i64) {
    GAS_REMAINING.store(gas, Ordering::SeqCst);
    if gas >= 0 {
        LAST_STATUS.store(0, Ordering::SeqCst);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_get_gas() -> i64 {
    GAS_REMAINING.load(Ordering::SeqCst)
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_charge_gas(amt: i64) -> i64 {
    let cur = GAS_REMAINING.fetch_sub(amt, Ordering::SeqCst);
    let rem = cur - amt;
    if rem < 0 {
        LAST_STATUS.store(crate::effect_abi::LUST_STATUS_ERR_OUT_OF_GAS as i64, Ordering::SeqCst);
    }
    rem
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_get_status() -> i64 {
    LAST_STATUS.load(Ordering::SeqCst)
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_set_status(status: i64) {
    LAST_STATUS.store(status, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_set_max_memory(bytes: i64) {
    MAX_MEMORY_LIMIT.store(bytes, Ordering::SeqCst);
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_get_memory_used() -> i64 {
    MEMORY_USED.load(Ordering::SeqCst)
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_alloc_array(len: i64) -> *mut i64 {
    let count = if len < 0 { 0 } else { len as usize };
    let size = (count + 2).max(2);
    let bytes_needed = (size * core::mem::size_of::<i64>()) as i64;
    let limit = MAX_MEMORY_LIMIT.load(Ordering::SeqCst);
    let prev = MEMORY_USED.fetch_add(bytes_needed, Ordering::SeqCst);
    if prev + bytes_needed > limit {
        MEMORY_USED.fetch_sub(bytes_needed, Ordering::SeqCst);
        LAST_STATUS.store(2, Ordering::SeqCst);
        panic!("Out of memory");
    }
    let mut vec = Vec::<i64>::with_capacity(size);
    vec.push(count as i64);
    vec.push(count as i64);
    vec.resize(size, 0);
    let slice = vec.into_boxed_slice();
    Box::into_raw(slice) as *mut i64
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_array_len(ptr: *const i64) -> i64 {
    if ptr.is_null() {
        return 0;
    }
    unsafe { *ptr }
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_array_get(ptr: *const i64, idx: i64) -> i64 {
    if ptr.is_null() {
        return 0;
    }
    let len = unsafe { *ptr };
    if idx < 0 || idx >= len {
        return 0;
    }
    unsafe { *ptr.offset(2 + idx as isize) }
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_array_set(ptr: *mut i64, idx: i64, val: i64) {
    if ptr.is_null() {
        return;
    }
    let len = unsafe { *ptr };
    if idx < 0 || idx >= len {
        return;
    }
    unsafe {
        *ptr.offset(2 + idx as isize) = val;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_div_int(a: i64, b: i64) -> i64 {
    if b == 0 {
        panic!("Division by zero");
    }
    if a == i64::MIN && b == -1 {
        i64::MIN
    } else {
        a / b
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_rem_int(a: i64, b: i64) -> i64 {
    if b == 0 {
        panic!("Modulo by zero");
    }
    if a == i64::MIN && b == -1 {
        0
    } else {
        a % b
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_floordiv_int(a: i64, b: i64) -> i64 {
    if b == 0 {
        panic!("Division by zero");
    }
    if a == i64::MIN && b == -1 {
        i64::MIN
    } else {
        let d = a / b;
        let rem = a % b;
        if (rem != 0) && ((rem < 0) != (b < 0)) {
            d - 1
        } else {
            d
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_pow_int(mut base: i64, exp: i64) -> i64 {
    if exp < 0 {
        panic!("Negative exponent in integer power");
    }
    let mut e = exp as u64;
    let mut res: i64 = 1;
    while e > 0 {
        if e & 1 == 1 {
            res = res.wrapping_mul(base);
        }
        base = base.wrapping_mul(base);
        e >>= 1;
    }
    res
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_print_int(x: i64) {
    println!("{x}");
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_abs_int(x: i64) -> i64 {
    x.saturating_abs()
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_min_int(a: i64, b: i64) -> i64 {
    a.min(b)
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_max_int(a: i64, b: i64) -> i64 {
    a.max(b)
}

#[unsafe(no_mangle)]
pub extern "C" fn lust_mir_clamp_int(x: i64, lo: i64, hi: i64) -> i64 {
    if lo <= hi {
        x.clamp(lo, hi)
    } else {
        x.clamp(hi, lo)
    }
}
