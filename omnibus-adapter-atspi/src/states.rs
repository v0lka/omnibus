//! AT-SPI state-set decoding: `GetState()` returns an array of u32 bit words;
//! state *n* lives in word *n / 32*, bit *n % 32*.
//!
//! The table follows the `AtspiStateType` enum from at-spi2-core
//! (`atspi-constants.h`): `ATSPI_STATE_INVALID` = 0 through
//! `ATSPI_STATE_READ_ONLY` = 43, verified against the installed headers.

/// Names ordered by bit position; index 0 (`invalid`) is never reported.
const STATE_NAMES: [&str; 44] = [
    "invalid",
    "active",
    "armed",
    "busy",
    "checked",
    "collapsed",
    "defunct",
    "editable",
    "enabled",
    "expandable",
    "expanded",
    "focusable",
    "focused",
    "has-tooltip",
    "horizontal",
    "iconified",
    "modal",
    "multi-line",
    "multiselectable",
    "opaque",
    "pressed",
    "resizable",
    "selectable",
    "selected",
    "sensitive",
    "showing",
    "single-line",
    "stale",
    "transient",
    "vertical",
    "visible",
    "manages-descendants",
    "indeterminate",
    "required",
    "truncated",
    "animated",
    "invalid-entry",
    "supports-autocompletion",
    "selectable-text",
    "is-default",
    "visited",
    "checkable",
    "has-popup",
    "read-only",
];

/// Decodes the state bit words into sorted state names; bits beyond the
/// known table are skipped (a newer server may define more states).
pub fn state_names(words: &[u32]) -> Vec<String> {
    let mut names = Vec::new();
    for (word_index, word) in words.iter().enumerate() {
        for bit in 0u32..32 {
            if word & (1 << bit) == 0 {
                continue;
            }
            let position = word_index * 32 + bit as usize;
            if let Some(name) = STATE_NAMES.get(position)
                && position != 0
            {
                names.push(String::from(*name));
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_single_word_bits() {
        // active = 1, editable = 7, enabled = 8, showing = 25.
        let words = [(1 << 1) | (1 << 7) | (1 << 8) | (1 << 25)];
        assert_eq!(
            state_names(&words),
            vec![
                String::from("active"),
                String::from("editable"),
                String::from("enabled"),
                String::from("showing")
            ]
        );
    }

    #[test]
    fn decodes_second_word_bits() {
        // checkable = 41 → word 1, bit 9; read-only = 43 → word 1, bit 11.
        let words = [0, (1 << 9) | (1 << 11)];
        assert_eq!(
            state_names(&words),
            vec![String::from("checkable"), String::from("read-only")]
        );
    }

    #[test]
    fn empty_and_unknown_bits_are_honest() {
        assert!(state_names(&[]).is_empty());
        assert!(state_names(&[0, 0]).is_empty());
        // Bits past the table are ignored, not fabricated (word 1, bit 31 = 63).
        assert!(state_names(&[0, 1 << 31]).is_empty());
    }

    #[test]
    fn the_invalid_bit_is_never_reported() {
        assert!(state_names(&[1]).is_empty());
    }
}
