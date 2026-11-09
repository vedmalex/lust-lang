//! Structural and textual diff formatter for Lust test assertions.

use alloc::{format, string::String};

/// Formats a visual diff between actual and expected values.
pub fn format_diff<T: core::fmt::Debug>(actual: &T, expected: &T) -> String {
    let actual_str = format!("{:#?}", actual);
    let expected_str = format!("{:#?}", expected);

    if actual_str == expected_str {
        return format!("Values appear identical in Debug representation:\n  {}", actual_str);
    }

    let mut out = String::new();
    out.push_str("\n--- Diff (- actual, + expected) ---\n");
    out.push_str(&format!("- Actual:   {}\n", actual_str.replace('\n', "\n  ")));
    out.push_str(&format!("+ Expected: {}\n", expected_str.replace('\n', "\n  ")));
    out
}

/// Formats a string value difference.
pub fn format_str_diff(actual: &str, expected: &str) -> String {
    let mut out = String::new();
    out.push_str("\n--- String Diff (- actual, + expected) ---\n");
    out.push_str(&format!("- Actual:   \"{}\"\n", actual));
    out.push_str(&format!("+ Expected: \"{}\"\n", expected));
    out
}
