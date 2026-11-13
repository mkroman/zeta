//! Small formatting helpers shared by the policy and the Markdown renderer.

use std::cmp::Ordering;

#[must_use]
pub fn number(value: Option<u64>) -> String {
    value.map_or_else(|| "—".to_owned(), group_digits)
}

#[must_use]
pub fn signed(value: Option<i64>) -> String {
    match value {
        None => "—".to_owned(),
        Some(0) => "0".to_owned(),
        Some(value) if value > 0 => format!("+{}", group_digits(value.unsigned_abs())),
        Some(value) => format!("-{}", group_digits(value.unsigned_abs())),
    }
}

/// Format an integer with thousands separators, for readable instruction counts.
#[must_use]
pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    grouped
}

#[must_use]
pub fn percent(value: Option<f64>) -> String {
    match value {
        None => "—".to_owned(),
        Some(value) if value.is_infinite() && value.is_sign_positive() => "new".to_owned(),
        Some(value) if value.is_infinite() => "gone".to_owned(),
        Some(value) if value.abs() < 0.005 => "0.00%".to_owned(),
        Some(value) => format!("{value:+.2}%"),
    }
}

/// Compare case ids naturally so `case_2` sorts before `case_10`.
#[must_use]
pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left = left.char_indices().peekable();
    let mut right = right.char_indices().peekable();

    loop {
        match (left.peek(), right.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(_), Some(_)) => {}
        }

        let left_digit = left
            .peek()
            .is_some_and(|(_, character)| character.is_ascii_digit());
        let right_digit = right
            .peek()
            .is_some_and(|(_, character)| character.is_ascii_digit());

        if left_digit && right_digit {
            let left_number = take_number(&mut left);
            let right_number = take_number(&mut right);
            match left_number.cmp(&right_number) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        } else {
            let (_, left_character) = left.next().expect("peeked");
            let (_, right_character) = right.next().expect("peeked");
            match left_character.cmp(&right_character) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }
    }
}

fn take_number(characters: &mut std::iter::Peekable<std::str::CharIndices<'_>>) -> u128 {
    let mut number = 0_u128;
    while let Some((_, character)) = characters.peek() {
        let Some(digit) = character.to_digit(10) else {
            break;
        };
        number = number.saturating_mul(10).saturating_add(u128::from(digit));
        characters.next();
    }
    number
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_values() {
        assert_eq!(number(Some(1234)), "1,234");
        assert_eq!(number(None), "—");
        assert_eq!(signed(Some(5)), "+5");
        assert_eq!(signed(Some(-5)), "-5");
        assert_eq!(signed(Some(0)), "0");
        assert_eq!(percent(Some(12.345)), "+12.35%");
        assert_eq!(percent(Some(0.0)), "0.00%");
        assert_eq!(percent(Some(-0.0)), "0.00%");
        assert_eq!(percent(Some(0.0001)), "0.00%");
        assert_eq!(percent(Some(-0.0001)), "0.00%");
        assert_eq!(percent(Some(f64::INFINITY)), "new");
    }

    #[test]
    fn sorts_case_ids_naturally() {
        let mut cases = ["case_10", "case_2", "case_1"];
        cases.sort_by(|left, right| natural_cmp(left, right));
        assert_eq!(cases, ["case_1", "case_2", "case_10"]);
    }
}
