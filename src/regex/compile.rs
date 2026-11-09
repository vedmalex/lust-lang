use crate::regex::plan::RegexPlan;
use crate::regex::policy::{MAX_CAPTURE_GROUPS, MAX_PATTERN_BYTES, RegexError};
use regex_automata::dfa::onepass::DFA;

pub fn compile_bounded(pattern: &str) -> Result<RegexPlan, RegexError> {
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(RegexError::PatternTooLarge(pattern.len()));
    }

    // Strict one-pass DFA compiler: any pattern requiring backtracking fails closed
    let dfa = DFA::new(pattern).map_err(|e| {
        RegexError::UnsupportedPattern(format!("{}: {}", pattern, e))
    })?;

    let fwd_dfa = regex_automata::hybrid::dfa::DFA::new(pattern).map_err(|e| {
        RegexError::UnsupportedPattern(format!("{}: {}", pattern, e))
    })?;

    let rev_dfa = regex_automata::hybrid::dfa::DFA::builder()
        .configure(
            regex_automata::hybrid::dfa::DFA::config()
                .prefilter(None)
                .specialize_start_states(false)
                .match_kind(regex_automata::MatchKind::All),
        )
        .thompson(regex_automata::nfa::thompson::Config::new().reverse(true))
        .build(pattern)
        .map_err(|e| {
            RegexError::UnsupportedPattern(format!("{}: {}", pattern, e))
        })?;

    let caps = dfa.create_captures();
    let group_count = caps.group_len();
    if group_count > MAX_CAPTURE_GROUPS {
        return Err(RegexError::TooManyCaptureGroups(group_count));
    }

    Ok(RegexPlan::new(pattern.to_string(), dfa, fwd_dfa, rev_dfa, group_count))
}
