use std::{collections::HashSet, fmt};

#[derive(Debug, PartialEq, Eq)]
pub enum SelectionError {
    EmptyDocument,
    InvalidRange { total: usize },
    Duplicate,
}

impl fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDocument => write!(formatter, "The PDF contains no pages."),
            Self::InvalidRange { total } => {
                write!(
                    formatter,
                    "Invalid page range. Use numbers within 1-{total}, e.g. 1,5-8."
                )
            }
            Self::Duplicate => write!(formatter, "The page selection must contain no duplicates."),
        }
    }
}

// Unicode 14.0 decimal zero code points, as used by Python 3.11's int().
// Each entry starts a contiguous range of ten decimal digits.
const DECIMAL_ZEROES: &[u32] = &[
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x16A60, 0x16AC0, 0x16B50,
    0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E950, 0x1FBF0,
];

fn decimal_digit(character: char) -> Option<usize> {
    let code = u32::from(character);
    let index = DECIMAL_ZEROES.partition_point(|&zero| zero <= code);
    let digit = code - DECIMAL_ZEROES[index.checked_sub(1)?];
    (digit < 10).then_some(digit as usize)
}

fn python_strip_whitespace(character: char) -> bool {
    character.is_whitespace() || matches!(character, '\u{001c}'..='\u{001f}')
}

fn parse_page_number(text: &str, max_digits: usize) -> Option<usize> {
    // int() accepts Unicode whitespace but excludes the four extra controls
    // that str.strip() removes from the outer segment.
    let text = text.trim_matches(char::is_whitespace);
    let digits = text.strip_prefix('+').unwrap_or(text);
    let mut value = 0usize;
    let mut digit_count = 0;
    let mut previous_was_digit = false;
    for character in digits.chars() {
        if character == '_' && previous_was_digit {
            previous_was_digit = false;
        } else {
            let digit = decimal_digit(character)?;
            digit_count += 1;
            if max_digits != 0 && digit_count > max_digits {
                return None;
            }
            value = value.checked_mul(10)?.checked_add(digit)?;
            previous_was_digit = true;
        }
    }
    previous_was_digit.then_some(value)
}

/// Select one-based PDF page numbers in the order requested by the caller.
/// Pass Python's `sys.get_int_max_str_digits()` as `max_digits`; zero disables the limit.
pub fn select_pages(
    selection: Option<&str>,
    total: usize,
    max_digits: usize,
) -> Result<Vec<usize>, SelectionError> {
    if total == 0 {
        return Err(SelectionError::EmptyDocument);
    }
    let Some(selection) =
        selection.filter(|text| !text.trim_matches(python_strip_whitespace).is_empty())
    else {
        return Ok((1..=total).collect());
    };

    let mut numbers = Vec::new();
    let mut seen = HashSet::new();
    for segment in selection.split(',') {
        let segment = segment.trim_matches(python_strip_whitespace);
        let (first, last) = segment.split_once('-').unwrap_or((segment, segment));
        let (Some(start), Some(end)) = (
            parse_page_number(first, max_digits),
            parse_page_number(last, max_digits),
        ) else {
            return Err(SelectionError::InvalidRange { total });
        };
        if start == 0 || start > end || end > total {
            return Err(SelectionError::InvalidRange { total });
        }
        for number in start..=end {
            if !seen.insert(number) {
                return Err(SelectionError::Duplicate);
            }
            numbers.push(number);
        }
    }
    Ok(numbers)
}

#[cfg(test)]
mod tests {
    use super::SelectionError;

    fn select_pages(selection: Option<&str>, total: usize) -> Result<Vec<usize>, SelectionError> {
        super::select_pages(selection, total, 4300)
    }

    #[test]
    fn python_strip_controls_are_removed_only_at_segment_edges() {
        for control in ['\u{001c}', '\u{001d}', '\u{001e}', '\u{001f}'] {
            assert_eq!(
                select_pages(Some(&control.to_string()), 3),
                Ok(vec![1, 2, 3])
            );
            assert_eq!(
                select_pages(Some(&format!("{control}1{control},{control}3{control}")), 3),
                Ok(vec![1, 3])
            );
            for selection in [format!("1{control}-3"), format!("1-{control}3")] {
                assert_eq!(
                    select_pages(Some(&selection), 3),
                    Err(SelectionError::InvalidRange { total: 3 })
                );
            }
        }
        assert_eq!(
            select_pages(Some("1\u{00a0}-\u{2003}3"), 3),
            Ok(vec![1, 2, 3])
        );
    }

    #[test]
    fn digit_limit_applies_to_each_bound_and_can_be_disabled() {
        let at_limit = format!("{}1", "0".repeat(4299));
        let over_limit = format!("{}1", "0".repeat(4300));
        assert_eq!(select_pages(Some(&at_limit), 3), Ok(vec![1]));
        for selection in [
            over_limit.clone(),
            format!("{over_limit}-3"),
            format!("1-{over_limit}"),
        ] {
            assert_eq!(
                select_pages(Some(&selection), 3),
                Err(SelectionError::InvalidRange { total: 3 })
            );
            assert_eq!(
                super::select_pages(Some(&selection), 3, 0),
                if selection.ends_with("-3") {
                    Ok(vec![1, 2, 3])
                } else {
                    Ok(vec![1])
                }
            );
        }
        assert_eq!(super::select_pages(Some("+０_１"), 3, 2), Ok(vec![1]));
        assert_eq!(
            super::select_pages(Some("+０_１"), 3, 1),
            Err(SelectionError::InvalidRange { total: 3 })
        );
    }

    #[test]
    fn omitted_selection_returns_all_pages() {
        assert_eq!(select_pages(None, 103), Ok((1..=103).collect()));
    }

    #[test]
    fn blank_selection_returns_all_pages() {
        for selection in ["", " \t "] {
            assert_eq!(select_pages(Some(selection), 3), Ok(vec![1, 2, 3]));
        }
    }

    #[test]
    fn mixed_selection_preserves_requested_order() {
        assert_eq!(select_pages(Some("85,2-4"), 103), Ok(vec![85, 2, 3, 4]));
    }

    #[test]
    fn overlapping_ranges_are_rejected() {
        assert_eq!(
            select_pages(Some("2-4,3"), 103),
            Err(SelectionError::Duplicate)
        );
    }

    #[test]
    fn invalid_ranges_are_rejected() {
        for selection in ["104", "0", "4-2", "1-x", "2,"] {
            assert_eq!(
                select_pages(Some(selection), 103),
                Err(SelectionError::InvalidRange { total: 103 }),
                "{selection}"
            );
        }
    }

    #[test]
    fn empty_document_is_rejected_before_selection() {
        for selection in [None, Some(""), Some("1")] {
            assert_eq!(
                select_pages(selection, 0),
                Err(SelectionError::EmptyDocument)
            );
        }
    }

    #[test]
    fn python_decimal_notation_is_accepted() {
        for selection in ["1_0", "１０", "+10"] {
            assert_eq!(select_pages(Some(selection), 103), Ok(vec![10]));
        }
        for selection in ["９-1_1", "+９-１１", "0_9-+11"] {
            assert_eq!(select_pages(Some(selection), 103), Ok(vec![9, 10, 11]));
        }
    }

    #[test]
    fn malformed_underscores_are_rejected_at_either_bound() {
        for selection in ["1__0", "_10", "10_", "_10-11", "10_-11", "9-1__0", "9-10_"] {
            assert_eq!(
                select_pages(Some(selection), 103),
                Err(SelectionError::InvalidRange { total: 103 }),
                "{selection}"
            );
        }
    }

    #[test]
    fn equivalent_decimal_spellings_are_duplicates() {
        assert_eq!(
            select_pages(Some("1_0,１０"), 103),
            Err(SelectionError::Duplicate)
        );
    }
}
