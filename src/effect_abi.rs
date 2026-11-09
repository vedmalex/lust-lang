use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Stable C-ABI version tag for first-class Lust effects (0x0001_0000 = v1.0).
pub const LUST_EFFECT_ABI_V1: u32 = 0x0001_0000;

/// Return / context status codes matching RFC v2.0 Section 2.4.
pub const LUST_STATUS_SUCCESS: u32 = 0;
pub const LUST_STATUS_ERR_GENERIC: u32 = 1;
pub const LUST_STATUS_ERR_OUT_OF_GAS: u32 = 2;
pub const LUST_STATUS_ERR_OUT_OF_MEMORY: u32 = 3;
pub const LUST_STATUS_ERR_INTERRUPTED: u32 = 4;

pub type LustStatus = u32;

/// C-ABI invocation function pointer type.
pub type LustInvokeFn = unsafe extern "C" fn(
    userdata: *mut core::ffi::c_void,
    rt: *mut core::ffi::c_void,
    env: *const core::ffi::c_void,
    out_exit: *mut core::ffi::c_void,
) -> LustStatus;

/// C-ABI release function pointer type for deterministic cleanup.
pub type LustReleaseFn = unsafe extern "C" fn(
    userdata: *mut core::ffi::c_void,
    rt: *mut core::ffi::c_void,
);

/// Escaping closure representation matching RFC Section 2.4.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LustClosureObject {
    pub code_ptr: *const (),
    pub env_layout_id: u32,
    pub capture_handle: *mut (),
}

/// Versioned C-ABI descriptor for first-class escaping Effect<A, E, R> and host interop (F04).
#[repr(C)]
pub struct LustEffectDescriptor {
    pub abi_version: u32,
    pub userdata: *mut core::ffi::c_void,
    pub invoke_fn: Option<LustInvokeFn>,
    pub release_fn: Option<LustReleaseFn>,
}

unsafe impl Send for LustEffectDescriptor {}
unsafe impl Sync for LustEffectDescriptor {}

impl Default for LustEffectDescriptor {
    fn default() -> Self {
        Self {
            abi_version: LUST_EFFECT_ABI_V1,
            userdata: core::ptr::null_mut(),
            invoke_fn: None,
            release_fn: None,
        }
    }
}

/// Internal termination cause for effect execution (F07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause<E = String> {
    /// Expected typed failure branch (E).
    Fail(E),
    /// Unchecked defect / fatal panic.
    Die(String),
    /// Terminal interruption (e.g. OutOfGas, hardware abort).
    /// Sticky and non-catchable by user catch blocks.
    Interrupt,
}

/// Disjoint result of executing an Effect<A, E, R>.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit<A, E = String> {
    Success(A),
    Failure(Cause<E>),
}

impl<A, E> Exit<A, E> {
    #[inline]
    pub fn is_success(&self) -> bool {
        matches!(self, Exit::Success(_))
    }

    #[inline]
    pub fn is_failure(&self) -> bool {
        matches!(self, Exit::Failure(_))
    }

    #[inline]
    pub fn is_interrupt(&self) -> bool {
        matches!(self, Exit::Failure(Cause::Interrupt))
    }

    /// Maps successful values preserving failures.
    pub fn map<B, F: FnOnce(A) -> B>(self, f: F) -> Exit<B, E> {
        match self {
            Exit::Success(a) => Exit::Success(f(a)),
            Exit::Failure(cause) => Exit::Failure(cause),
        }
    }

    /// Catches expected `Cause::Fail(e)`.
    /// INVARIANT: `Cause::Interrupt` and `Cause::Die` are STICKY and cannot be caught.
    pub fn catch<F: FnOnce(E) -> Exit<A, E>>(self, f: F) -> Exit<A, E> {
        match self {
            Exit::Failure(Cause::Fail(e)) => f(e),
            other => other,
        }
    }

    /// Converts exit status to canonical C-ABI status code.
    pub fn status_code(&self) -> LustStatus {
        match self {
            Exit::Success(_) => LUST_STATUS_SUCCESS,
            Exit::Failure(Cause::Interrupt) => LUST_STATUS_ERR_INTERRUPTED,
            Exit::Failure(Cause::Die(_)) => LUST_STATUS_ERR_GENERIC,
            Exit::Failure(Cause::Fail(_)) => LUST_STATUS_ERR_GENERIC,
        }
    }
}

/// Stack-allocated context projection adapter (F03).
/// Allows zero-heap-allocation projection of environment records (e.g. Env_ABC -> Env_AC).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct StackContextProjection<const N: usize> {
    pub service_ptrs: [*const core::ffi::c_void; N],
    pub service_tags: [u32; N],
}

impl<const N: usize> StackContextProjection<N> {
    #[inline]
    pub const fn new(service_ptrs: [*const core::ffi::c_void; N], service_tags: [u32; N]) -> Self {
        Self {
            service_ptrs,
            service_tags,
        }
    }

    #[inline]
    pub fn get(&self, tag: u32) -> Option<*const core::ffi::c_void> {
        let mut i = 0;
        while i < N {
            if self.service_tags[i] == tag {
                return Some(self.service_ptrs[i]);
            }
            i += 1;
        }
        None
    }
}

/// RAII transaction ledger for layered service assembly (F13).
/// If layer composition fails, all previously acquired services are finalized in reverse (LIFO) order.
pub struct LayerRollbackLedger {
    finalizers: Vec<Box<dyn FnOnce() + 'static>>,
    committed: bool,
}

impl LayerRollbackLedger {
    pub fn new() -> Self {
        Self {
            finalizers: Vec::new(),
            committed: false,
        }
    }

    /// Registers a finalizer for an acquired layer service.
    pub fn register<F: FnOnce() + 'static>(&mut self, finalizer: F) {
        self.finalizers.push(Box::new(finalizer));
    }

    /// Commit the layer assembly, preventing rollback on drop.
    pub fn commit(&mut self) {
        self.committed = true;
    }

    /// Explicitly trigger rollback, executing all registered finalizers in reverse (LIFO) order.
    pub fn rollback(&mut self) {
        self.committed = true; // prevent double rollback on drop
        while let Some(f) = self.finalizers.pop() {
            f();
        }
    }

    pub fn count(&self) -> usize {
        self.finalizers.len()
    }
}

impl Default for LayerRollbackLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LayerRollbackLedger {
    fn drop(&mut self) {
        if !self.committed {
            while let Some(f) = self.finalizers.pop() {
                f();
            }
        }
    }
}
