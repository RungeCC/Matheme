//! Convenience macros for building Wolfram association rules and expressions.
//!
//! ```
//! use matheme_wl::{assoc, rule, Expr};
//!
//! let expr = assoc! {
//!     "Status" -> "Success",
//!     "Pages" -> 2_i64,
//!     "Items" -> Expr::list([1_i64, 2]),
//! };
//! let rules = expr.as_association().expect("association");
//! assert_eq!(rules.len(), 3);
//! assert_eq!(rule!("Key" -> true).key().as_str(), Some("Key"));
//! ```
//!
//! Keys are string literals (the shape used by every bridge association) and
//! values are arbitrary expressions. A trailing comma is accepted.

/// Build one immediate rule, `key -> value`.
#[macro_export]
macro_rules! rule {
    ($key:literal -> $value:expr $(,)?) => {
        $crate::Rule::new($key, $value)
    };
}

/// Build a `Vec<Rule>` from `key -> value` entries.
#[macro_export]
macro_rules! rules {
    ($($key:literal -> $value:expr),* $(,)?) => {
        vec![$($crate::rule!($key -> $value)),*]
    };
}

/// Build an Association expression from `key -> value` entries.
#[macro_export]
macro_rules! assoc {
    ($($key:literal -> $value:expr),* $(,)?) => {
        $crate::Expr::association([$($crate::rule!($key -> $value)),*])
    };
}

#[cfg(test)]
mod tests {
    use crate::{Expr, Rule};

    #[test]
    fn rule_builds_an_immediate_rule() {
        let rule = crate::rule!("Key" -> 7_i64);
        assert_eq!(rule.key().as_str(), Some("Key"));
        assert_eq!(rule.value().as_i64(), Some(7));
    }

    #[test]
    fn rules_builds_vec_of_immediate_rules() {
        let list = crate::rules![
            "Status" -> "Success",
            "Pages" -> 3_i64,
        ];
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].key().as_str(), Some("Status"));
        assert_eq!(list[0].value().as_str(), Some("Success"));
        assert_eq!(list[1].key().as_str(), Some("Pages"));
        assert_eq!(list[1].value().as_i64(), Some(3));
    }

    #[test]
    fn assoc_builds_nested_values_and_accepts_a_trailing_comma() {
        let expr = crate::assoc! {
            "Name" -> "matheme",
            "Pages" -> 2_i64,
            "Items" -> Expr::list([1_i64, 2, 3]),
            "Flag" -> true,
        };
        let rules = expr.as_association().expect("association");
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0].key().and_then(Expr::as_str), Some("Name"));
        assert_eq!(rules[1].pair().expect("rule").1.as_i64(), Some(2));
        assert_eq!(
            rules[2]
                .pair()
                .expect("rule")
                .1
                .as_list()
                .expect("list")
                .len(),
            3
        );
        assert_eq!(rules[3].pair().expect("rule").1.as_bool(), Some(true));
    }

    #[test]
    fn empty_assoc_has_no_entries() {
        let expr = crate::assoc!();
        assert!(expr.as_association().expect("association").is_empty());
    }

    #[test]
    fn assoc_values_can_be_rules() {
        let expr = crate::assoc! { "Nested" -> Expr::association([Rule::new("A", 1_i64)]) };
        let rules = expr.as_association().expect("association");
        let nested = rules[0]
            .pair()
            .expect("rule")
            .1
            .as_association()
            .expect("nested");
        assert_eq!(nested.len(), 1);
    }
}
