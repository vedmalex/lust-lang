use alloc::vec::Vec;
use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeaseError {
    TenantMismatch { expected: u32, actual: u32 },
    StaleGeneration { expected: u32, actual: u32 },
    InvalidState(&'static str),
    ReleaseBeforeJoin,
    SlotNotFound,
}

impl fmt::Display for LeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaseError::TenantMismatch { expected, actual } => {
                write!(f, "Tenant mismatch: expected {}, got {}", expected, actual)
            }
            LeaseError::StaleGeneration { expected, actual } => {
                write!(f, "Stale generation: expected {}, got {}", expected, actual)
            }
            LeaseError::InvalidState(msg) => write!(f, "Invalid lease state: {}", msg),
            LeaseError::ReleaseBeforeJoin => {
                write!(f, "Protocol violation: release invoked before join acknowledgement")
            }
            LeaseError::SlotNotFound => write!(f, "Owner slot not found"),
        }
    }
}

/// 16-byte owner identifier tracking runtime, tenant, slot, and generation
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerId {
    pub runtime_id: u32,
    pub tenant_id: u32,
    pub slot_id: u32,
    pub generation: u32,
}

impl OwnerId {
    pub const fn new(runtime_id: u32, tenant_id: u32, slot_id: u32, generation: u32) -> Self {
        Self {
            runtime_id,
            tenant_id,
            slot_id,
            generation,
        }
    }

    pub const fn dummy() -> Self {
        Self {
            runtime_id: 0,
            tenant_id: 0,
            slot_id: 0,
            generation: 1,
        }
    }
}

impl fmt::Display for OwnerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OwnerId(rt:{}, tenant:{}, slot:{}, gen:{})",
            self.runtime_id, self.tenant_id, self.slot_id, self.generation
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseLifecycleState {
    Active,
    Cancelled,
    Joined,
    Released,
}

#[derive(Debug, Clone)]
pub struct SlotEntry {
    pub tenant_id: u32,
    pub generation: u32,
    pub state: LeaseLifecycleState,
}

/// Registry managing scoped leases, generations, and cancel -> join -> release protocol
#[derive(Debug, Clone, Default)]
pub struct OwnerRegistry {
    runtime_id: u32,
    slots: Vec<SlotEntry>,
}

impl OwnerRegistry {
    pub fn new(runtime_id: u32) -> Self {
        Self {
            runtime_id,
            slots: Vec::new(),
        }
    }

    pub fn allocate(&mut self, tenant_id: u32) -> OwnerId {
        // Look for an existing released slot to reuse
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.state == LeaseLifecycleState::Released {
                slot.generation = slot.generation.wrapping_add(1);
                slot.tenant_id = tenant_id;
                slot.state = LeaseLifecycleState::Active;
                return OwnerId::new(self.runtime_id, tenant_id, idx as u32, slot.generation);
            }
        }

        let slot_id = self.slots.len() as u32;
        let generation = 1;
        self.slots.push(SlotEntry {
            tenant_id,
            generation,
            state: LeaseLifecycleState::Active,
        });
        OwnerId::new(self.runtime_id, tenant_id, slot_id, generation)
    }

    /// Pre-dereference check: validates runtime, tenant, generation, and active state strictly before memory access
    pub fn validate_access(&self, owner: OwnerId, current_tenant: u32) -> Result<(), LeaseError> {
        if owner.runtime_id != self.runtime_id {
            return Err(LeaseError::InvalidState("OwnerId issued by foreign runtime"));
        }

        let slot = self
            .slots
            .get(owner.slot_id as usize)
            .ok_or(LeaseError::SlotNotFound)?;

        if slot.tenant_id != current_tenant {
            return Err(LeaseError::TenantMismatch {
                expected: current_tenant,
                actual: slot.tenant_id,
            });
        }

        if owner.tenant_id != current_tenant {
            return Err(LeaseError::TenantMismatch {
                expected: current_tenant,
                actual: owner.tenant_id,
            });
        }

        if slot.generation != owner.generation {
            return Err(LeaseError::StaleGeneration {
                expected: slot.generation,
                actual: owner.generation,
            });
        }

        if slot.state != LeaseLifecycleState::Active {
            return Err(LeaseError::InvalidState("Slot is not active"));
        }

        Ok(())
    }

    /// Step 1 of invalidation: signal cancellation
    pub fn cancel(&mut self, owner: OwnerId) -> Result<(), LeaseError> {
        if owner.runtime_id != self.runtime_id {
            return Err(LeaseError::InvalidState("OwnerId issued by foreign runtime"));
        }

        let slot = self
            .slots
            .get_mut(owner.slot_id as usize)
            .ok_or(LeaseError::SlotNotFound)?;

        if slot.generation != owner.generation {
            return Err(LeaseError::StaleGeneration {
                expected: slot.generation,
                actual: owner.generation,
            });
        }

        if slot.state != LeaseLifecycleState::Active {
            return Err(LeaseError::InvalidState("Cannot cancel non-active slot"));
        }

        slot.state = LeaseLifecycleState::Cancelled;
        Ok(())
    }

    /// Step 2 of invalidation: acknowledge child task join
    pub fn acknowledge_join(&mut self, owner: OwnerId) -> Result<(), LeaseError> {
        if owner.runtime_id != self.runtime_id {
            return Err(LeaseError::InvalidState("OwnerId issued by foreign runtime"));
        }

        let slot = self
            .slots
            .get_mut(owner.slot_id as usize)
            .ok_or(LeaseError::SlotNotFound)?;

        if slot.generation != owner.generation {
            return Err(LeaseError::StaleGeneration {
                expected: slot.generation,
                actual: owner.generation,
            });
        }

        if slot.state != LeaseLifecycleState::Cancelled {
            return Err(LeaseError::InvalidState(
                "Cannot acknowledge join on un-cancelled slot",
            ));
        }

        slot.state = LeaseLifecycleState::Joined;
        Ok(())
    }

    /// Step 3 of invalidation: release buffer memory (strictly requires Joined state)
    pub fn release(&mut self, owner: OwnerId) -> Result<(), LeaseError> {
        if owner.runtime_id != self.runtime_id {
            return Err(LeaseError::InvalidState("OwnerId issued by foreign runtime"));
        }

        let slot = self
            .slots
            .get_mut(owner.slot_id as usize)
            .ok_or(LeaseError::SlotNotFound)?;

        if slot.generation != owner.generation {
            return Err(LeaseError::StaleGeneration {
                expected: slot.generation,
                actual: owner.generation,
            });
        }

        if slot.state != LeaseLifecycleState::Joined {
            return Err(LeaseError::ReleaseBeforeJoin);
        }

        slot.state = LeaseLifecycleState::Released;
        Ok(())
    }

    pub fn slot_state(&self, slot_id: u32) -> Option<LeaseLifecycleState> {
        self.slots.get(slot_id as usize).map(|s| s.state)
    }
}
