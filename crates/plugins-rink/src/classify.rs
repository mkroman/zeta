//! Unit-casing classification of the evaluated expression.
//!
//! Rink resolves unit names case-sensitively, so `.c` rewrites the tokens that do not resolve
//! into their registered spelling before evaluation. The correction runs on rink's parsed query
//! rather than on the raw input, so number-and-unit words like `10usd` still go through rink's
//! own tokenizer.

use rink_core::Context as RinkContext;
use rink_core::ast::{Conversion, Expr, Query};
use rink_core::output::fmt::TokenFmt;
use rink_core::parsing::text_query::{TokenIterator, parse_query};

/// Case-corrects the unit tokens in `line` that rink cannot resolve.
///
/// The correction runs on rink's parsed query rather than the input string, so number-and-
/// unit words like `10usd` are handled by rink's own tokenizer. Each `Expr::Unit` name that
/// does not resolve is matched case-insensitively against the registry's names — defined
/// units, substances, base units and pending dependencies — and replaced when exactly one
/// candidate exists. Queries that end up untouched, including ones the parser cannot make
/// sense of, are returned unchanged.
pub fn classify(ctx: &RinkContext, line: &str) -> String {
    let mut iter = TokenIterator::new(line.trim()).peekable();
    let mut query = parse_query(&mut iter);

    if fix_unit_casing(ctx, &mut query) {
        query.spans_to_string()
    } else {
        line.to_owned()
    }
}

/// Rewrites every `Expr::Unit` name in `query` that rink cannot resolve into its
/// case-corrected spelling, when exactly one registry name matches case-insensitively.
///
/// Names are drawn from the registry's defined units, substances, base units, their long forms
/// and pending dependencies — so lowercase ISO codes like `usd` correct to `USD` both while the
/// currency data is missing and once it is loaded. A candidate that equals the name itself is
/// not a rewrite: such a query is untouched. Returns whether anything was rewritten; an
/// untouched query must be evaluated verbatim, since re-rendering it could shift a parse that
/// only rink's own error handling can report correctly. A query that already failed to parse
/// is always left verbatim too — re-rendering it would flatten rink's structured error into
/// literal `<error: …>` text, which then reads as a missing unit.
fn fix_unit_casing(ctx: &RinkContext, query: &mut Query) -> bool {
    /// Returns the single registry name matching `name` case-insensitively.
    ///
    /// Candidates are the documented set: defined units, pending dependencies, substances and
    /// base units with their long forms. `None` means the name already matches (nothing to
    /// rewrite), nothing matched, or the match is ambiguous.
    fn resolve_case<'ctx>(ctx: &'ctx RinkContext, name: &str) -> Option<&'ctx str> {
        let mut candidates: Vec<&str> = ctx
            .registry
            .units
            .keys()
            .map(String::as_str)
            .chain(ctx.registry.missing_deps.keys().map(String::as_str))
            .chain(ctx.registry.substances.keys().map(String::as_str))
            .chain(ctx.registry.base_unit_long_names.keys().map(String::as_str))
            .chain(ctx.registry.base_units.iter().map(|unit| unit.id.as_str()))
            .filter(|candidate| candidate.eq_ignore_ascii_case(name))
            .collect();

        candidates.sort_unstable();
        candidates.dedup();

        match candidates.as_slice() {
            [only] if *only != name => Some(*only),
            _ => None,
        }
    }

    fn fix_expr(ctx: &RinkContext, expr: &mut Expr, fixed: &mut bool) {
        match expr {
            Expr::Unit { name } => {
                if ctx.lookup(name).is_none()
                    && let Some(resolved) = resolve_case(ctx, name)
                {
                    name.clear();
                    name.push_str(resolved);
                    *fixed = true;
                }
            }
            Expr::BinOp(binop) => {
                fix_expr(ctx, &mut binop.left, fixed);
                fix_expr(ctx, &mut binop.right, fixed);
            }
            Expr::UnaryOp(unaryop) => fix_expr(ctx, &mut unaryop.expr, fixed),
            Expr::Mul { exprs } => {
                for expr in exprs {
                    fix_expr(ctx, expr, fixed);
                }
            }
            Expr::Of { expr, .. } => fix_expr(ctx, expr, fixed),
            Expr::Call { args, .. } => {
                for arg in args {
                    fix_expr(ctx, arg, fixed);
                }
            }
            Expr::Quote { .. } | Expr::Const { .. } | Expr::Date { .. } | Expr::Error { .. } => {}
        }
    }

    // A parse error makes the evaluation fail whatever the casing, and re-rendering would
    // turn rink's structured error into literal `<error: …>` text — leave the line alone so
    // rink reports the real parse error.
    if contains_parse_error(query) {
        return false;
    }

    let mut fixed = false;

    let (top, conversion) = match query {
        Query::Expr(expr) | Query::Factorize(expr) | Query::UnitsFor(expr) => (expr, None),
        Query::Convert(top, conversion, ..) => (top, Some(conversion)),
        Query::Search(_) | Query::Error(_) => return fixed,
    };

    fix_expr(ctx, top, &mut fixed);

    match conversion {
        Some(Conversion::Expr(bottom)) => fix_expr(ctx, bottom, &mut fixed),
        // A unit list (`1000 -> ft, in`) carries bare names instead of expressions, so it
        // needs its own pass: skipping it would correct the left-hand side only and report the
        // uncorrected list item as a missing unit.
        Some(Conversion::List(units)) => {
            for unit in units {
                if ctx.lookup(unit).is_none()
                    && let Some(resolved) = resolve_case(ctx, unit)
                {
                    unit.clear();
                    unit.push_str(resolved);
                    fixed = true;
                }
            }
        }
        _ => {}
    }

    fixed
}

/// Returns whether `query` carries a parse error: a top-level [`Query::Error`] or an
/// `Expr::Error` nested in an otherwise valid query.
///
/// Such a query fails evaluation however its units are cased, and re-rendering it would turn
/// rink's structured error message into literal text.
fn contains_parse_error(query: &Query) -> bool {
    fn expr_contains_error(expr: &Expr) -> bool {
        match expr {
            Expr::Error { .. } => true,
            Expr::BinOp(binop) => {
                expr_contains_error(&binop.left) || expr_contains_error(&binop.right)
            }
            Expr::UnaryOp(unaryop) => expr_contains_error(&unaryop.expr),
            Expr::Mul { exprs } => exprs.iter().any(expr_contains_error),
            Expr::Of { expr, .. } => expr_contains_error(expr),
            Expr::Call { args, .. } => args.iter().any(expr_contains_error),
            Expr::Quote { .. } | Expr::Const { .. } | Expr::Date { .. } | Expr::Unit { .. } => {
                false
            }
        }
    }

    match query {
        Query::Expr(expr) | Query::Factorize(expr) | Query::UnitsFor(expr) => {
            expr_contains_error(expr)
        }
        Query::Convert(top, conversion, ..) => {
            expr_contains_error(top)
                || matches!(conversion, Conversion::Expr(bottom) if expr_contains_error(bottom))
        }
        Query::Search(_) => false,
        Query::Error(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::currency::build_context;
    use crate::test_support::{currency_server, evaluate, offline_plugin, test_plugin};
    use zeta_utils::irc::strip_formatting;

    #[tokio::test]
    async fn classify_case_corrects_unresolvable_units() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        rink.ensure_currency_cached().await;

        let cases = [
            ("10 usd to dkk", "149.504 DKK"),
            ("10usd to dkk", "149.504 DKK"),
            ("10 USD to DKK", "149.504 DKK"),
            ("30 eur to jpy", "5115.6 JPY"),
        ];

        for (line, value) in cases {
            let reply = strip_formatting(&evaluate(&rink, line, true).await);
            assert!(reply.contains(value), "{line}: missing {value:?}: {reply}");
            assert!(reply.contains("money"), "{line}: {reply}");
        }
    }

    #[tokio::test]
    async fn classify_corrects_units_while_the_currency_data_is_missing() {
        let rink = offline_plugin();

        // The corrections land before any evaluation: the reply names the *tracked*
        // dependencies (USD), which only the rewritten `10 USD to DKK` can trigger.
        let reply = strip_formatting(&evaluate(&rink, "10 usd to dkk", true).await);
        assert!(reply.contains("Missing dependencies: USD"), "{reply}");
    }

    #[tokio::test]
    async fn classify_leaves_unclassifiable_tokens_alone() {
        let rink = offline_plugin();

        let cases = [
            ("50 gram", &["50 gram", "mass"][..]),
            ("banana", &["Error: No such unit banana"][..]),
            (
                "2 hours from now",
                &["No such unit from", "did you mean freon?"][..],
            ),
        ];

        for (line, expected) in cases {
            let reply = strip_formatting(&evaluate(&rink, line, true).await);

            for needle in expected {
                assert!(
                    reply.contains(needle),
                    "{line}: missing {needle:?}: {reply}"
                );
            }
        }
    }

    #[tokio::test]
    async fn classify_returns_unparseable_queries_unchanged() {
        let rink = offline_plugin();

        let reply = strip_formatting(&evaluate(&rink, "20:00 in tokyo", true).await);
        assert!(reply.contains("Error: Expected term, got `:`"), "{reply}");
    }

    #[tokio::test]
    async fn classify_reports_the_parser_error_of_a_malformed_query() {
        let rink = offline_plugin();

        // Re-rendering the parsed query would flatten the parser's error into a literal
        // `<error: …>` unit, so the line stays verbatim and rink reports the real problem.
        for line in ["egg $%^", "10 usd +", "5 usd -> dkk + "] {
            assert_eq!(
                evaluate(&rink, line, true).await,
                evaluate(&rink, line, false).await,
                "classification changed the outcome of {line:?}"
            );
        }
    }

    #[tokio::test]
    async fn classify_only_reports_rewrites_that_change_the_name() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));
        rink.ensure_currency_cached().await;

        // A bare `Option::cloned` is ambiguous between the `&T` and `&mut T` impls.
        #[allow(clippy::redundant_closure_for_method_calls)]
        let data = rink
            .currency
            .read(|data| data.cloned())
            .expect("the fetched currency data must be cached");
        let ctx = build_context(Some(&data), None).expect("the fetched data must build a context");

        let rewritten = |line: &str| {
            let mut iter = TokenIterator::new(line.trim()).peekable();
            let mut query = parse_query(&mut iter);

            fix_unit_casing(&ctx, &mut query)
        };

        // A context lookup that misses a substance is not a rewrite: `egg` resolves through
        // the substance table during evaluation, so nothing about the name changes.
        assert!(!rewritten("egg"), "a no-op must not count as a rewrite");
        assert!(!rewritten("banana"), "no candidate means no rewrite");
        assert!(
            !rewritten("10 USD"),
            "an already correct name is not a rewrite"
        );
        assert!(rewritten("10usd"), "a genuine case mismatch is a rewrite");
    }

    #[tokio::test]
    async fn classify_corrects_the_units_of_a_unit_list() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));
        rink.ensure_currency_cached().await;

        // Both list items are corrected, not just the left-hand side.
        assert_eq!(
            evaluate(&rink, "5 usd -> dkk, eur", true).await,
            evaluate(&rink, "5 USD -> DKK, EUR", false).await,
        );
    }

    #[tokio::test]
    async fn classification_leaves_every_other_expression_form_unchanged() {
        let rink = offline_plugin();

        // Expression forms from rink's manual; classification must not change the outcome of
        // any of them. Clock-dependent forms are excluded: their output legitimately differs
        // between the two evaluations.
        for line in [
            "10.1e2",
            "0x10",
            "0o10",
            "0b10",
            "1_000",
            "1e100",
            "3 4 m 5 s",
            "10 km / 5 m",
            "1|2 m",
            "12 meters + 5 feet",
            "12 ft^2",
            "meter mod foot",
            "1 << 24",
            "0b1010 and 0b1100 to base 2",
            "12 degC -> °F",
            "2000 kcal -> potato = 164 kcal",
            "12 'core' hour / 3 'core' -> minutes",
            "5 millimeters",
            "3 kibibytes",
            "2 quarters",
            "meter -> feet",
            "1000 -> hex",
            "10000 -> base 36",
            "2^128 -> digits",
            "mass of electron -> eng",
            "3 foot -> frac",
            "2^17 seconds -> hour;min;sec",
            "units for power",
            "units power",
            "factorize velocity",
            "search milk",
            "#jan 01, 1970#",
            "#2016-08-24# + 500 weeks",
            "#today#",
            "#apr 1, 2016 12:00:00 +01:00#",
            "milk",
            "gallon milk",
            "egg",
            "egg_shelled of kg egg",
            "gallon gasoline -> btu",
            "sqrt(2)",
            "fac(5)",
            // failures must stay identical too
            "banana",
            "2 hours from now",
            "20:00 in tokyo",
            "1 << meter",
        ] {
            let strict = evaluate(&rink, line, false).await;
            let smart = evaluate(&rink, line, true).await;
            assert_eq!(strict, smart, "classification changed {line:?}");
        }
    }
}
