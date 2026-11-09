
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    BeforeInsert,
    AfterInsert,
    BeforeUpdate,
    AfterUpdate,
    BeforeDelete,
    AfterDelete,
}

#[derive(Debug, Clone, Default)]
pub struct LifecycleHooks {
    pub registered_hooks: Vec<HookEvent>,
}

impl LifecycleHooks {
    pub fn new() -> Self {
        Self {
            registered_hooks: Vec::new(),
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
}
