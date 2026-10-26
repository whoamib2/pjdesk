//! Grouping for a device ID the user is typing.
//!
//! The field stores the grouped text (`902 488 771`). Each edit is rewritten
//! from the digits only, and the caret is put back after the same digit, so
//! inserting a space does not move the next keystroke into the wrong group.
//! Backspace that removes only a group separator also removes the digit
//! before that separator.

/// Groups of three from the left, at most `max_digits` digits.
pub fn format_typed_id(raw: &str) -> String {
    let (text, _) = reformat_id_change("", raw, raw.chars().count(), 9);
    text
}

/// `before` is the field before the edit, `after` is what the widget produced,
/// and `after_caret` is the caret as a character index in `after`.
/// Returns the grouped text and the caret as a character index in that text.
pub fn reformat_id_change(before: &str, after: &str, after_caret: usize, max_digits: usize) -> (String, usize) {
    let max_digits = max_digits.max(1);
    let before_digits: Vec<char> = before.chars().filter(|c| c.is_ascii_digit()).collect();
    let after_digits: Vec<char> = after.chars().filter(|c| c.is_ascii_digit()).collect();
    let (prefix, removed) = removed_span(before, after);
    let separator_only = before_digits == after_digits
        && !removed.is_empty()
        && removed.chars().all(|c| matches!(c, ' ' | '-' | '_' | '.'));

    let mut digits = after_digits;
    let mut caret_digits = digits_before(after, after_caret);
    if separator_only {
        let at = digits_before(before, prefix);
        if at > 0 && at - 1 < digits.len() {
            digits.remove(at - 1);
            caret_digits = at - 1;
        }
    }
    if digits.len() > max_digits {
        digits.truncate(max_digits);
    }
    caret_digits = caret_digits.min(digits.len());
    let formatted = group_left(&digits);
    let caret = char_index_after_digits(&formatted, caret_digits);
    (formatted, caret)
}

fn group_left(digits: &[char]) -> String {
    let mut out = String::new();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(' ');
        }
        out.push(*c);
    }
    out
}

fn digits_before(text: &str, caret: usize) -> usize {
    text.chars().take(caret).filter(|c| c.is_ascii_digit()).count()
}

fn char_index_after_digits(formatted: &str, digits: usize) -> usize {
    if digits == 0 {
        return 0;
    }
    let mut seen = 0;
    for (index, ch) in formatted.chars().enumerate() {
        if ch.is_ascii_digit() {
            seen += 1;
            if seen == digits {
                return index + 1;
            }
        }
    }
    formatted.chars().count()
}

/// Character index where the edit starts, and the characters removed from `before`.
fn removed_span(before: &str, after: &str) -> (usize, String) {
    let before: Vec<char> = before.chars().collect();
    let after: Vec<char> = after.chars().collect();
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix + prefix < before.len() && suffix + prefix < after.len() && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let end = before.len() - suffix;
    let removed: String = before[prefix..end].iter().collect();
    (prefix, removed)
}

/// Applies one inserted character at `caret` (a character index) and reformats.
pub fn type_char(text: &str, caret: usize, ch: char, max_digits: usize) -> (String, usize) {
    let mut chars: Vec<char> = text.chars().collect();
    let at = caret.min(chars.len());
    chars.insert(at, ch);
    let after: String = chars.into_iter().collect();
    reformat_id_change(text, &after, at + 1, max_digits)
}

/// Deletes the character before `caret`, then reformats.
pub fn backspace(text: &str, caret: usize, max_digits: usize) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    if caret == 0 || chars.is_empty() {
        return (text.to_string(), caret.min(chars.len()));
    }
    let at = caret.min(chars.len());
    let mut next = chars;
    next.remove(at - 1);
    let after: String = next.into_iter().collect();
    reformat_id_change(text, &after, at - 1, max_digits)
}

/// Replaces the whole field, as select-all plus paste or type does.
pub fn replace_all(before: &str, pasted: &str, max_digits: usize) -> (String, usize) {
    reformat_id_change(before, pasted, pasted.chars().count(), max_digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_all(keys: &str) -> (String, usize) {
        let mut text = String::new();
        let mut caret = 0;
        for ch in keys.chars() {
            let (next, next_caret) = type_char(&text, caret, ch, 9);
            text = next;
            caret = next_caret;
        }
        (text, caret)
    }

    #[test]
    fn typing_nine_digits_groups_from_the_left() {
        let (text, caret) = type_all("902488771");
        assert_eq!(text, "902 488 771");
        assert_eq!(caret, text.chars().count());
    }

    #[test]
    fn caret_stays_after_the_digit_just_typed() {
        let (text, caret) = type_all("9024");
        assert_eq!(text, "902 4");
        assert_eq!(caret, text.chars().count());
        let (text, caret) = type_char("902", 3, '4', 9);
        assert_eq!(text, "902 4");
        assert_eq!(caret, 5);
    }

    #[test]
    fn paste_ignores_spaces_and_dashes() {
        let (text, caret) = replace_all("", "902-488-771", 9);
        assert_eq!(text, "902 488 771");
        assert_eq!(caret, text.chars().count());
        let (text, caret) = replace_all("", "902 488 771", 9);
        assert_eq!(text, "902 488 771");
        assert_eq!(caret, text.chars().count());
        let (text, _) = replace_all("111 222 333", "90 24 88 771 extra", 9);
        assert_eq!(text, "902 488 771");
    }

    #[test]
    fn backspace_across_a_group_space_deletes_the_digit() {
        let (text, caret) = backspace("902 488", 4, 9);
        assert_eq!(text, "904 88");
        assert_eq!(caret, 2);
    }

    #[test]
    fn backspace_of_a_digit_keeps_the_caret_on_that_digit() {
        let (text, caret) = backspace("902 488", 7, 9);
        assert_eq!(text, "902 48");
        assert_eq!(caret, text.chars().count());
    }

    #[test]
    fn editing_in_the_middle_inserts_at_the_caret() {
        let (text, caret) = type_char("902 488 771", 4, '5', 9);
        assert_eq!(text, "902 548 877");
        assert_eq!(caret, 5);
        assert_eq!(&text[..caret], "902 5");
    }

    #[test]
    fn select_all_and_type_replaces_the_id() {
        let (text, caret) = replace_all("902 488 771", "1", 9);
        assert_eq!(text, "1");
        assert_eq!(caret, 1);
    }

    #[test]
    fn non_digits_are_ignored_and_the_tenth_digit_is_dropped() {
        let (text, caret) = type_all("90a24887719");
        assert_eq!(text, "902 488 771");
        assert_eq!(caret, text.chars().count());
    }

    #[test]
    fn longer_ids_can_be_allowed() {
        let (text, caret) = reformat_id_change("", "123456789012", 12, 12);
        assert_eq!(text, "123 456 789 012");
        assert_eq!(caret, text.chars().count());
    }
}
