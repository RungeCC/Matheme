//! Convenience macros for building Wolfram expressions, lists, normal calls,
//! rules, and associations.
//!
//! ```
//! use matheme_wl::{assoc, graphics, fundamental, list, normal, rule};
//!
//! let expr = assoc! {
//!     "Status" -> "Success",
//!     "Pages" -> 2_i64,
//!     "Items" -> list![1_i64, "two", true],
//! };
//! let rules = expr.as_association().expect("association");
//! assert_eq!(rules.len(), 3);
//! assert_eq!(rule!("Key" -> true).key().as_str(), Some("Key"));
//!
//! let g = normal!(
//!     &graphics::GRAPHICS,
//!     list![],
//!     &graphics::IMAGE_SIZE => list![100.0_f64, 50.0_f64],
//!     &graphics::IMAGE_PADDING => &fundamental::NONE,
//! );
//! assert!(g.as_normal().unwrap().has_head(&graphics::GRAPHICS));
//! ```
//!
//! - [`rule!`] accepts `"Key" -> value` (for string-keyed associations) or
//!   `lhs => rhs` (for arbitrary expression keys, such as `Symbol` options).
//! - [`rules!`] builds a `Vec<Rule>`.
//! - [`assoc!`] builds a ``System`Association`` expression.
//! - [`list!`] builds a ``System`List`` expression with heterogeneous elements.
//! - [`normal!`] builds a `head[args…]` normal expression with heterogeneous
//!   arguments and optional inline `lhs => rhs` ``System`Rule`` entries.

/// Build one immediate rule (`Rule`), either `"key" -> value` or `lhs => rhs`.
///
/// `Rule` implements `Into<Expr>` (producing ``System`Rule[lhs, rhs]``), so it
/// can be used both inside [`assoc!`] / `Expr::association` and directly as an
/// argument to [`normal!`] or [`list!`].
#[macro_export]
macro_rules! rule {
    ($key:literal -> $value:expr $(,)?) => {
        $crate::Rule::new($key, $value)
    };
    ($key:expr => $value:expr $(,)?) => {
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

/// Build a ``System`List`` expression from zero or more heterogeneous elements,
/// converting each element independently via `Expr::from`.
#[macro_export]
macro_rules! list {
    () => {
        $crate::Expr::list(::std::iter::empty::<$crate::Expr>())
    };
    ($($elem:expr),+ $(,)?) => {
        $crate::Expr::list([$($crate::Expr::from($elem)),+])
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __wl_push_normal_args {
    ($vec:ident $(,)?) => {};
    ($vec:ident, $lhs:expr => $rhs:expr $(, $($rest:tt)*)?) => {
        $vec.push($crate::Expr::from($crate::Rule::new($lhs, $rhs)));
        $($crate::__wl_push_normal_args!($vec, $($rest)*);)?
    };
    ($vec:ident, $arg:expr $(, $($rest:tt)*)?) => {
        $vec.push($crate::Expr::from($arg));
        $($crate::__wl_push_normal_args!($vec, $($rest)*);)?
    };
}

/// Build a normal expression `head[args…]` with zero or more heterogeneous
/// arguments.
///
/// Arguments may be arbitrary expressions convertible via `Into<Expr>`, or
/// inline `lhs => rhs` option pairs (converted to ``System`Rule[lhs, rhs]``).
///
/// ```
/// # use matheme_wl::{fundamental, graphics, list, normal};
/// let g = normal!(
///     &graphics::GRAPHICS,
///     list![],
///     &graphics::ASPECT_RATIO => &fundamental::AUTOMATIC,
/// );
/// assert_eq!(g.as_normal().unwrap().len(), 2);
/// ```
#[macro_export]
macro_rules! normal {
    ($head:expr $(,)?) => {
        $crate::Expr::normal($head, ::std::iter::empty::<$crate::Expr>())
    };
    ($head:expr, $($arg:expr),+ $(,)?) => {
        $crate::Expr::normal($head, [$($crate::Expr::from($arg)),+])
    };
    ($head:expr, $($args:tt)+) => {{
        let mut __wl_args = ::std::vec::Vec::<$crate::Expr>::new();
        $crate::__wl_push_normal_args!(__wl_args, $($args)+);
        $crate::Expr::normal($head, __wl_args)
    }};
}

#[cfg(test)]
mod tests {
    use crate::{Expr, Rule, Symbol, fundamental, graphics};

    #[test]
    fn rule_builds_an_immediate_rule() {
        let rule = crate::rule!("Key" -> 7_i64);
        assert_eq!(rule.key().as_str(), Some("Key"));
        assert_eq!(rule.value().as_i64(), Some(7));

        let sym_rule = crate::rule!(&graphics::IMAGE_PADDING => &fundamental::NONE);
        let as_expr = Expr::from(sym_rule);
        let n = as_expr.as_normal().expect("Rule expr");
        assert!(n.has_head(&Symbol::RULE));
        assert_eq!(
            n.get(0).and_then(Expr::as_symbol),
            Some("System`ImagePadding")
        );
        assert_eq!(n.get(1).and_then(Expr::as_symbol), Some("System`None"));
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
            "Items" -> crate::list![1_i64, 2_i64, 3_i64],
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

    #[test]
    fn list_macro_supports_empty_and_heterogeneous_elements() {
        let empty = crate::list![];
        assert!(empty.as_list().expect("empty list").is_empty());

        let mixed = crate::list!["Miter", 4.0_f64, 2_i64, &fundamental::NONE, true];
        let elems = mixed.as_list().expect("mixed list");
        assert_eq!(elems.len(), 5);
        assert_eq!(elems[0].as_str(), Some("Miter"));
        assert_eq!(elems[1].as_f64(), Some(4.0));
        assert_eq!(elems[2].as_i64(), Some(2));
        assert!(elems[3].is_symbol(&fundamental::NONE));
        assert_eq!(elems[4].as_bool(), Some(true));
    }

    #[test]
    fn normal_macro_supports_zero_args_heterogeneous_args_and_inline_rules() {
        let zero = crate::normal!(&graphics::GRAPHICS);
        let n0 = zero.as_normal().expect("0-arg normal");
        assert!(n0.has_head(&graphics::GRAPHICS));
        assert!(n0.is_empty());

        let plain = crate::normal!(
            &graphics::DASHING,
            crate::list![0.1_f64, 0.05_f64],
            0.03_f64,
            "Round"
        );
        let np = plain.as_normal().expect("plain normal");
        assert!(np.has_head(&graphics::DASHING));
        assert_eq!(np.len(), 3);
        assert_eq!(np.get(1).and_then(Expr::as_f64), Some(0.03));
        assert_eq!(np.get(2).and_then(Expr::as_str), Some("Round"));

        let with_opts = crate::normal!(
            &graphics::GRAPHICS,
            crate::list![],
            &graphics::IMAGE_SIZE => crate::list![100.0_f64, 50.0_f64],
            &graphics::PLOT_RANGE_CLIPPING => true,
            &graphics::IMAGE_PADDING => &fundamental::NONE,
        );
        let nw = with_opts.as_normal().expect("normal with options");
        assert!(nw.has_head(&graphics::GRAPHICS));
        assert_eq!(nw.len(), 4);
        let opt1 = nw.get(1).unwrap().as_normal().expect("Rule");
        assert!(opt1.has_head(&Symbol::RULE));
        assert_eq!(
            opt1.get(0).and_then(Expr::as_symbol),
            Some("System`ImageSize")
        );
        let opt2 = nw.get(2).unwrap().as_normal().expect("Rule");
        assert_eq!(opt2.get(1).and_then(Expr::as_bool), Some(true));
        let opt3 = nw.get(3).unwrap().as_normal().expect("Rule");
        assert_eq!(opt3.get(1).and_then(Expr::as_symbol), Some("System`None"));
    }
}
