use crate::data::Document;
use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    BeforeInsert,
    AfterInsert,
    BeforeUpdate,
    AfterUpdate,
    BeforeDelete,
    AfterDelete,
    BeforeSave,
    AfterLoad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookAction {
    Continue,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookError {
    pub message: String,
}

impl HookError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HookError: {}", self.message)
    }
}

impl std::error::Error for HookError {}

pub type BeforeSaveDelegate =
    Arc<dyn Fn(&mut Document) -> Result<HookAction, HookError> + Send + Sync>;
pub type AfterLoadDelegate = Arc<dyn Fn(&mut Document) -> Result<(), HookError> + Send + Sync>;

#[derive(Clone, Default)]
pub struct LifecycleHooks {
    pub registered_hooks: Vec<HookEvent>,
    pub before_save_delegates: Vec<BeforeSaveDelegate>,
    pub after_load_delegates: Vec<AfterLoadDelegate>,
}

impl fmt::Debug for LifecycleHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LifecycleHooks")
            .field("registered_hooks", &self.registered_hooks)
            .field("before_save_count", &self.before_save_delegates.len())
            .field("after_load_count", &self.after_load_delegates.len())
            .finish()
    }
}

impl LifecycleHooks {
    pub fn new() -> Self {
        Self {
            registered_hooks: Vec::new(),
            before_save_delegates: Vec::new(),
            after_load_delegates: Vec::new(),
        }
    }

    pub fn register(&mut self, event: HookEvent) {
        if !self.registered_hooks.contains(&event) {
            self.registered_hooks.push(event);
        }
    }

    pub fn has_hook(&self, event: &HookEvent) -> bool {
        self.registered_hooks.contains(event)
    }

    pub fn add_before_save<F>(&mut self, hook: F)
    where
        F: Fn(&mut Document) -> Result<HookAction, HookError> + Send + Sync + 'static,
    {
        self.register(HookEvent::BeforeSave);
        self.before_save_delegates.push(Arc::new(hook));
    }

    pub fn add_after_load<F>(&mut self, hook: F)
    where
        F: Fn(&mut Document) -> Result<(), HookError> + Send + Sync + 'static,
    {
        self.register(HookEvent::AfterLoad);
        self.after_load_delegates.push(Arc::new(hook));
    }

    /// Execute before_save delegates in fixed FIFO registration order.
    /// Returns Ok(HookAction::Cancel) if any hook cancels (write must be skipped).
    /// Returns Err(HookError) if any hook fails.
    pub fn run_before_save(&self, doc: &mut Document) -> Result<HookAction, HookError> {
        for hook in &self.before_save_delegates {
            match hook(doc)? {
                HookAction::Continue => {}
                HookAction::Cancel => return Ok(HookAction::Cancel),
            }
        }
        Ok(HookAction::Continue)
    }

    /// Execute after_load delegates in fixed FIFO registration order.
    /// Returns Err(HookError) if any hook fails.
    pub fn run_after_load(&self, doc: &mut Document) -> Result<(), HookError> {
        for hook in &self.after_load_delegates {
            hook(doc)?;
        }
        Ok(())
    }
}
