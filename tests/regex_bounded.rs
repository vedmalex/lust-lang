use lust::regex::{compile_bounded, BoundedRegex, RegexError, RegexWorkTracker};

#[test]
fn test_regex_valid_onepass_match() {
    let re = BoundedRegex::new(r"^[0-9]+$").unwrap();
    assert_eq!(re.is_match("123456").unwrap(), true);
    assert_eq!(re.is_match("1234a56").unwrap(), false);

    let re_email = BoundedRegex::new(r"^[a-z]+@[a-z]+\.[a-z]+$").unwrap();
    assert_eq!(re_email.is_match("user@example.com").unwrap(), true);
    assert_eq!(re_email.is_match("invalid-email").unwrap(), false);
}

#[test]
fn test_gate_04_backtracking_pattern_triggers_unsupported() {
    // Patterns with features requiring backtracking or unsupported by onepass DFA (e.g. lookaround)
    // must strictly trigger UnsupportedPattern without falling back to an NFA or backtracking engine
    let lookahead_pattern = r"(?=abc)abc";
    let res = compile_bounded(lookahead_pattern);
    assert!(res.is_err());
    match res.unwrap_err() {
        RegexError::UnsupportedPattern(p) => {
            assert!(p.contains("(?=abc)abc"));
        }
        other => panic!("Expected UnsupportedPattern, got {:?}", other),
    }

    // Backreferences (e.g. \1) are strictly unsupported in DFA
    let backref_pattern = r"([a-z])\1";
    let res = compile_bounded(backref_pattern);
    assert!(matches!(res, Err(RegexError::UnsupportedPattern(_))));
}

#[test]
fn test_regex_linear_work_budget_scaling() {
    // Test linear scaling O(n) using work tracker formula
    let re = BoundedRegex::new(r"^[a-z]+$").unwrap();

    let n1 = 100;
    let s1 = "a".repeat(n1);
    assert!(re.is_match(&s1).unwrap());

    let n2 = 200;
    let s2 = "a".repeat(n2);
    assert!(re.is_match(&s2).unwrap());

    let tracker1 = RegexWorkTracker::new(n1, 1, 0).unwrap();
    let tracker2 = RegexWorkTracker::new(n2, 1, 0).unwrap();

    // Budget ratio should be within (n2 + 1) / (n1 + 1)
    let ratio = tracker2.max_budget as f64 / tracker1.max_budget as f64;
    assert!((ratio - 2.0).abs() < 0.05, "Work budget must scale strictly linearly");
}

#[test]
fn test_regex_find_and_replace() {
    let re = BoundedRegex::new(r"[0-9]+").unwrap();
    let m = re.find("order: 42 items").unwrap().unwrap();
    assert_eq!(m.text, "42");
    assert_eq!(m.start, 7);
    assert_eq!(m.end, 9);

    let replaced = re.replace("count: 100", "999").unwrap();
    assert_eq!(replaced, "count: 999");
}

#[test]
fn test_regex_pattern_size_limit() {
    let huge_pattern = "a".repeat(70_000);
    let res = compile_bounded(&huge_pattern);
    assert!(matches!(res, Err(RegexError::PatternTooLarge(_))));
}

#[test]
fn test_regex_find_on_empty_input_does_not_panic() {
    let re_star = BoundedRegex::new("a*").unwrap();
    let m = re_star.find("").unwrap();
    assert!(m.is_some());
    let m = m.unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 0);
    assert_eq!(m.text, "");

    let re_plus = BoundedRegex::new("a+").unwrap();
    assert_eq!(re_plus.find("").unwrap(), None);
}

#[test]
fn test_regex_find_1byte_match_at_eof_does_not_panic() {
    let re = BoundedRegex::new("z").unwrap();
    let m = re.find("z").unwrap().unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 1);
    assert_eq!(m.text, "z");

    let m2 = re.find("prefix-z").unwrap().unwrap();
    assert_eq!(m2.start, 7);
    assert_eq!(m2.end, 8);
    assert_eq!(m2.text, "z");
}

#[test]
fn test_regex_replace_all_no_match_stays_linear() {
    let re = BoundedRegex::new("xyz").unwrap();
    let text = "hello world without matches";
    let res = re.replace_all(text, "replacement").unwrap();
    assert_eq!(res, text);

    let multiple = BoundedRegex::new(r"\d+").unwrap();
    let res_mult = multiple.replace_all("a 1 b 22 c 333", "N").unwrap();
    assert_eq!(res_mult, "a N b N c N");
}
