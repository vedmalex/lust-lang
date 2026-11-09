use regex_automata::dfa::onepass::DFA;

#[derive(Debug, Clone)]
pub struct RegexPlan {
    pub pattern: String,
    pub dfa: DFA,
    pub group_count: usize,
}

impl RegexPlan {
    pub fn new(pattern: String, dfa: DFA, group_count: usize) -> Self {
        Self {
            pattern,
            dfa,
            group_count,
        }
    }
}
