use crate::regex::plan::RegexPlan;
use crate::regex::policy::{RegexError, RegexWorkTracker};
use regex_automata::Input;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexMatch {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub fn is_match(plan: &RegexPlan, input_text: &str) -> Result<bool, RegexError> {
    let mut tracker = RegexWorkTracker::new(input_text.len(), plan.group_count, 0)?;
    let mut caps = plan.dfa.create_captures();
    let mut cache = plan.dfa.create_cache();

    // Onepass DFA requires Anchored::Yes
    let input = Input::new(input_text).anchored(regex_automata::Anchored::Yes);
    tracker.charge_scanned(input_text.len() as u64)?;
    tracker.charge_transitions(input_text.len() as u64)?;

    plan.dfa.try_search(&mut cache, &input, &mut caps).map_err(|e| {
        RegexError::SyntaxError(e.to_string())
    })?;

    if caps.is_match() {
        tracker.charge_capture_updates(plan.group_count as u64)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn find(plan: &RegexPlan, input_text: &str) -> Result<Option<RegexMatch>, RegexError> {
    let mut tracker = RegexWorkTracker::new(input_text.len(), plan.group_count, 0)?;

    // Linear-time forward DFA pre-check: scans input once in O(n) to find match existence and end position
    let mut fwd_cache = plan.fwd_dfa.create_cache();
    let fwd_input = Input::new(input_text);
    tracker.charge_scanned(input_text.len() as u64)?;
    tracker.charge_transitions(input_text.len() as u64)?;

    let half_match = plan
        .fwd_dfa
        .try_search_fwd(&mut fwd_cache, &fwd_input)
        .map_err(|e| RegexError::SyntaxError(e.to_string()))?;

    let hm = match half_match {
        Some(m) => m,
        None => return Ok(None),
    };

    let mut caps = plan.dfa.create_captures();
    let mut cache = plan.dfa.create_cache();

    // Match is guaranteed to end at hm.offset(), scan candidate start positions up to hm.offset()
    for pos in 0..=hm.offset() {
        let input = Input::new(input_text)
            .range(pos..)
            .anchored(regex_automata::Anchored::Yes);

        tracker.charge_scanned(1)?;
        tracker.charge_transitions(1)?;

        plan.dfa.try_search(&mut cache, &input, &mut caps).map_err(|e| {
            RegexError::SyntaxError(e.to_string())
        })?;

        if let Some(span) = caps.get_group(0) {
            tracker.charge_capture_updates(1)?;
            let matched_text = &input_text[span.start..span.end];
            return Ok(Some(RegexMatch {
                start: span.start,
                end: span.end,
                text: matched_text.to_string(),
            }));
        }
    }

    Ok(None)
}

pub fn replace_all(
    plan: &RegexPlan,
    input_text: &str,
    replacement: &str,
) -> Result<String, RegexError> {
    let max_output_estimate = input_text.len().saturating_add(replacement.len().saturating_mul(10));
    let mut tracker = RegexWorkTracker::new(input_text.len(), plan.group_count, max_output_estimate)?;

    // Linear-time forward DFA pre-check: if no matches exist anywhere, return input in O(n)
    let mut fwd_cache = plan.fwd_dfa.create_cache();
    let fwd_input = Input::new(input_text);
    tracker.charge_scanned(input_text.len() as u64)?;
    tracker.charge_transitions(input_text.len() as u64)?;

    let has_any_match = plan
        .fwd_dfa
        .try_search_fwd(&mut fwd_cache, &fwd_input)
        .map_err(|e| RegexError::SyntaxError(e.to_string()))?;

    if has_any_match.is_none() {
        return Ok(input_text.to_string());
    }

    let mut caps = plan.dfa.create_captures();
    let mut cache = plan.dfa.create_cache();
    let mut out = String::new();
    let mut last_idx = 0;

    let mut pos = 0;
    while pos <= input_text.len() {
        let input = Input::new(input_text)
            .range(pos..)
            .anchored(regex_automata::Anchored::Yes);

        tracker.charge_scanned(1)?;
        tracker.charge_transitions(1)?;

        plan.dfa.try_search(&mut cache, &input, &mut caps).map_err(|e| {
            RegexError::SyntaxError(e.to_string())
        })?;

        if let Some(span) = caps.get_group(0) {
            if span.start >= span.end && span.start == pos && pos > last_idx {
                pos += 1;
                continue;
            }
            tracker.charge_capture_updates(1)?;
            let prefix = &input_text[last_idx..span.start];
            out.push_str(prefix);
            tracker.charge_copied(prefix.len() as u64)?;

            out.push_str(replacement);
            tracker.charge_copied(replacement.len() as u64)?;

            last_idx = span.end;
            pos = span.end.max(pos + 1);
        } else {
            pos += 1;
        }
    }

    let suffix = &input_text[last_idx..];
    out.push_str(suffix);
    tracker.charge_copied(suffix.len() as u64)?;

    Ok(out)
}
