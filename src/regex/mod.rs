pub mod compile;
pub mod execute;
pub mod plan;
pub mod policy;

pub use compile::compile_bounded;
pub use execute::{find, is_match, replace_all, RegexMatch};
pub use plan::RegexPlan;
pub use policy::{RegexError, RegexWorkTracker};

/// Bounded regex wrapper implementing safe DFA execution.
#[derive(Debug, Clone)]
pub struct BoundedRegex {
    plan: RegexPlan,
}

impl BoundedRegex {
    pub fn new(pattern: &str) -> Result<Self, RegexError> {
        let plan = compile_bounded(pattern)?;
        Ok(Self { plan })
    }

    pub fn is_match(&self, text: &str) -> Result<bool, RegexError> {
        is_match(&self.plan, text)
    }

    pub fn find(&self, text: &str) -> Result<Option<RegexMatch>, RegexError> {
        find(&self.plan, text)
    }

    pub fn replace(&self, text: &str, replacement: &str) -> Result<String, RegexError> {
        replace_all(&self.plan, text, replacement)
    }
}
