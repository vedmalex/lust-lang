//! Zero-Heap Mocking and Context Injection via StackContextProjection.

use alloc::{string::{String, ToString}, vec::Vec};
use crate::effect_abi::StackContextProjection;

#[cfg(feature = "std")]
use std::sync::Mutex;
#[cfg(not(feature = "std"))]
use core::cell::RefCell;

/// Record of an invoked method on a MockService.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockCall {
    pub method: String,
    pub args: Vec<String>,
    pub timestamp_ms: u64,
}

/// Dynamic mock service with call spy and response configuration.
pub struct MockService {
    pub service_tag: u32,
    #[cfg(feature = "std")]
    calls: Mutex<Vec<MockCall>>,
    #[cfg(feature = "std")]
    responses: Mutex<hashbrown::HashMap<String, Vec<String>>>,
    #[cfg(not(feature = "std"))]
    calls: RefCell<Vec<MockCall>>,
    #[cfg(not(feature = "std"))]
    responses: RefCell<hashbrown::HashMap<String, Vec<String>>>,
}

impl core::fmt::Debug for MockService {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MockService")
            .field("service_tag", &self.service_tag)
            .finish()
    }
}

impl MockService {
    /// Creates a new mock service with the given unique service tag.
    pub fn new(service_tag: u32) -> Self {
        Self {
            service_tag,
            #[cfg(feature = "std")]
            calls: Mutex::new(Vec::new()),
            #[cfg(feature = "std")]
            responses: Mutex::new(hashbrown::HashMap::new()),
            #[cfg(not(feature = "std"))]
            calls: RefCell::new(Vec::new()),
            #[cfg(not(feature = "std"))]
            responses: RefCell::new(hashbrown::HashMap::new()),
        }
    }

    /// Records an invocation of a method with arguments and timestamp.
    pub fn record_call(&self, method: &str, args: &[&str], timestamp_ms: u64) {
        let call = MockCall {
            method: method.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            timestamp_ms,
        };
        #[cfg(feature = "std")]
        {
            let mut list = self.calls.lock().unwrap();
            list.push(call);
        }
        #[cfg(not(feature = "std"))]
        {
            self.calls.borrow_mut().push(call);
        }
    }

    /// Returns the number of times `method` was invoked.
    pub fn call_count(&self, method: &str) -> usize {
        #[cfg(feature = "std")]
        {
            let list = self.calls.lock().unwrap();
            list.iter().filter(|c| c.method == method).count()
        }
        #[cfg(not(feature = "std"))]
        {
            self.calls.borrow().iter().filter(|c| c.method == method).count()
        }
    }

    /// Returns the most recent call to `method`, if any.
    pub fn last_call(&self, method: &str) -> Option<MockCall> {
        #[cfg(feature = "std")]
        {
            let list = self.calls.lock().unwrap();
            list.iter().rev().find(|c| c.method == method).cloned()
        }
        #[cfg(not(feature = "std"))]
        {
            self.calls.borrow().iter().rev().find(|c| c.method == method).cloned()
        }
    }

    /// Configures a stubbed return value for a method.
    pub fn set_response(&self, method: &str, response: String) {
        #[cfg(feature = "std")]
        {
            let mut map = self.responses.lock().unwrap();
            map.entry(method.to_string()).or_default().push(response);
        }
        #[cfg(not(feature = "std"))]
        {
            self.responses.borrow_mut().entry(method.to_string()).or_default().push(response);
        }
    }

    /// Exports the service as a tuple `(service_ptr, service_tag)` ready for
    /// zero-heap stack projection via `StackContextProjection<N>`.
    #[inline]
    pub fn as_projection_entry(&self) -> (*const core::ffi::c_void, u32) {
        (self as *const Self as *const core::ffi::c_void, self.service_tag)
    }

    /// Helper to construct a `StackContextProjection<1>` from a single mock.
    pub fn to_projection_single(&self) -> StackContextProjection<1> {
        let (ptr, tag) = self.as_projection_entry();
        StackContextProjection::new([ptr], [tag])
    }

    /// Helper to construct a `StackContextProjection<2>` from two mocks.
    pub fn to_projection_pair(first: &MockService, second: &MockService) -> StackContextProjection<2> {
        let (p1, t1) = first.as_projection_entry();
        let (p2, t2) = second.as_projection_entry();
        StackContextProjection::new([p1, p2], [t1, t2])
    }
}
