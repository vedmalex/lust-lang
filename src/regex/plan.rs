use regex_automata::dfa::onepass::DFA as OnepassDFA;
use regex_automata::hybrid::dfa::DFA as HybridDFA;

#[derive(Debug, Clone)]
pub struct RegexPlan {
    pub pattern: String,
    pub dfa: OnepassDFA,
    pub fwd_dfa: HybridDFA,
    pub group_count: usize,
}

impl RegexPlan {
    pub fn new(pattern: String, dfa: OnepassDFA, fwd_dfa: HybridDFA, group_count: usize) -> Self {
        Self {
            pattern,
            dfa,
            fwd_dfa,
            group_count,
        }
    }
}
