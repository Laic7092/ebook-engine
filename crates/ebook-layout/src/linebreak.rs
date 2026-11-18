use ebook_ir::WordBreak;

use crate::linebreak_table::{lb_of, Lb};
use crate::measure::is_wide;

pub(crate) fn can_break(a: char, b: char, word_break: WordBreak) -> bool {
    if b == '\n' {
        return true;
    }
    if word_break == WordBreak::BreakAll {
        return true;
    }
    if word_break == WordBreak::KeepAll {
        return a == ' ';
    }

    let raw_a = lb_of(a);
    let mut ca = raw_a;
    let cb = lb_of(b);

    if raw_a == Lb::Cr && cb == Lb::Lf {
        return false;
    }
    if matches!(raw_a, Lb::Bk | Lb::Cr | Lb::Lf | Lb::Nl) {
        return true;
    }
    if matches!(cb, Lb::Bk | Lb::Cr | Lb::Lf | Lb::Nl) {
        return false;
    }
    if matches!(cb, Lb::Sp | Lb::Zw) {
        return false;
    }
    if raw_a == Lb::Zw {
        return true;
    }
    if raw_a == Lb::Zwj {
        return false;
    }
    if matches!(cb, Lb::Cm | Lb::Zwj) {
        return false;
    }
    if ca == Lb::Cm {
        ca = Lb::Al;
    }

    if ca == Lb::Wj || cb == Lb::Wj {
        return false;
    }
    if ca == Lb::Gl {
        return false;
    }
    if cb == Lb::Gl && !matches!(ca, Lb::Sp | Lb::Hy | Lb::Hh) {
        return false;
    }
    if matches!(cb, Lb::Cl | Lb::Cp | Lb::Ex | Lb::Sy) {
        return false;
    }
    if ca == Lb::Op {
        return false;
    }
    if cb == Lb::Is {
        return false;
    }
    if ca == Lb::B2 && cb == Lb::B2 {
        return false;
    }
    if ca == Lb::Sp {
        return true;
    }

    if matches!(cb, Lb::Qu | Lb::Qf) {
        return false;
    }
    if cb == Lb::Qi && !is_wide(a) {
        return false;
    }
    if matches!(ca, Lb::Qu | Lb::Qi) {
        return false;
    }

    if ca == Lb::Cb || cb == Lb::Cb {
        return true;
    }

    if matches!(cb, Lb::Ba | Lb::Hh | Lb::Hy | Lb::Ns) {
        return false;
    }
    if ca == Lb::Bb {
        return false;
    }
    if ca == Lb::Sy && cb == Lb::Hl {
        return false;
    }
    if cb == Lb::In {
        return false;
    }

    if matches!(ca, Lb::Al | Lb::Hl) && cb == Lb::Nu {
        return false;
    }
    if ca == Lb::Nu && matches!(cb, Lb::Al | Lb::Hl) {
        return false;
    }
    if ca == Lb::Pr && matches!(cb, Lb::Id | Lb::Eb | Lb::Em) {
        return false;
    }
    if matches!(ca, Lb::Id | Lb::Eb | Lb::Em) && cb == Lb::Po {
        return false;
    }
    if matches!(ca, Lb::Pr | Lb::Po) && matches!(cb, Lb::Al | Lb::Hl) {
        return false;
    }
    if matches!(ca, Lb::Al | Lb::Hl) && matches!(cb, Lb::Pr | Lb::Po) {
        return false;
    }
    if matches!(ca, Lb::Po | Lb::Pr) && matches!(cb, Lb::Op | Lb::Nu) {
        return false;
    }
    if ca == Lb::Hy && cb == Lb::Nu {
        return false;
    }
    if matches!(ca, Lb::Nu | Lb::Sy | Lb::Is) && cb == Lb::Nu {
        return false;
    }
    if matches!(ca, Lb::Nu | Lb::Sy | Lb::Is) && matches!(cb, Lb::Po | Lb::Pr) {
        return false;
    }
    if matches!(ca, Lb::Cl | Lb::Cp) && matches!(cb, Lb::Po | Lb::Pr) {
        return false;
    }

    if ca == Lb::Jl && matches!(cb, Lb::Jl | Lb::Jv | Lb::H2 | Lb::H3) {
        return false;
    }
    if matches!(ca, Lb::Jv | Lb::H2) && matches!(cb, Lb::Jv | Lb::Jt) {
        return false;
    }
    if matches!(ca, Lb::Jt | Lb::H3) && cb == Lb::Jt {
        return false;
    }
    if matches!(ca, Lb::Jl | Lb::Jv | Lb::Jt | Lb::H2 | Lb::H3) && cb == Lb::Po {
        return false;
    }
    if ca == Lb::Pr && matches!(cb, Lb::Jl | Lb::Jv | Lb::Jt | Lb::H2 | Lb::H3) {
        return false;
    }

    if matches!(ca, Lb::Al | Lb::Hl) && matches!(cb, Lb::Al | Lb::Hl) {
        return false;
    }
    if ca == Lb::Ap && (matches!(cb, Lb::Ak | Lb::As) || b == '\u{25CC}') {
        return false;
    }
    if (matches!(ca, Lb::Ak | Lb::As) || a == '\u{25CC}') && matches!(cb, Lb::Vf | Lb::Vi) {
        return false;
    }
    if ca == Lb::Is && matches!(cb, Lb::Al | Lb::Hl) {
        return false;
    }

    if matches!(ca, Lb::Al | Lb::Hl | Lb::Nu) && cb == Lb::Op && !is_wide(b) {
        return false;
    }
    if ca == Lb::Cp && !is_wide(a) && matches!(cb, Lb::Al | Lb::Hl | Lb::Nu) {
        return false;
    }
    if ca == Lb::Eb && cb == Lb::Em {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linebreak_table::{RANGES, RANGE_COUNT};

    fn normal(a: char, b: char) -> bool {
        can_break(a, b, WordBreak::Normal)
    }

    #[test]
    fn table_is_sorted_and_merged() {
        assert_eq!(RANGE_COUNT, RANGES.len());
        for pair in RANGES.windows(2) {
            assert!(pair[0].0 <= pair[0].1);
            assert!(pair[0].1 < pair[1].0);
            if pair[0].2 == pair[1].2 {
                assert!(pair[0].1 + 1 < pair[1].0);
            }
        }
        assert!(RANGES.iter().all(|range| range.2 != Lb::Al));
        assert_eq!(lb_of('\u{4E00}'), Lb::Id);
        assert_eq!(lb_of('a'), Lb::Al);
        assert_eq!(lb_of('\u{00A0}'), Lb::Gl);
        assert_eq!(lb_of('\u{200B}'), Lb::Zw);
        assert_eq!(lb_of('\u{2060}'), Lb::Wj);
        assert_eq!(lb_of('\u{0301}'), Lb::Cm);
    }

    #[test]
    fn nbsp_glues_both_sides() {
        assert!(!normal('a', '\u{00A0}'));
        assert!(!normal('\u{00A0}', 'a'));
        assert!(!normal('\u{00A0}', '\u{00A0}'));
    }

    #[test]
    fn zero_width_space_breaks_after() {
        assert!(normal('\u{200B}', 'a'));
        assert!(!normal('a', '\u{200B}'));
    }

    #[test]
    fn word_joiner_glues_both_sides() {
        assert!(!normal('a', '\u{2060}'));
        assert!(!normal('\u{2060}', 'a'));
        assert!(!normal('\u{2060}', '\u{2060}'));
    }

    #[test]
    fn combining_mark_stays_with_base() {
        assert!(!normal('e', '\u{0301}'));
        assert!(!normal('\u{0301}', '\u{0301}'));
        assert!(!normal('e', '\u{200D}'));
    }

    #[test]
    fn numbers_and_units_stay_together() {
        assert!(!normal('1', '0'));
        assert!(!normal('0', 'k'));
        assert!(!normal('k', 'g'));
        assert!(!normal('0', '%'));
        assert!(!normal('3', '.'));
        assert!(!normal('.', '5'));
        assert!(!normal('1', '/'));
        assert!(!normal('/', '2'));
    }

    #[test]
    fn cjk_punctuation_avoids_line_edges() {
        assert!(!normal('\u{4F60}', '\u{3002}'));
        assert!(!normal('\u{4F60}', '\u{FF0C}'));
        assert!(!normal('\u{4F60}', '\u{300D}'));
        assert!(!normal('\u{4F60}', '\u{FF09}'));
        assert!(!normal('\u{300C}', '\u{4F60}'));
        assert!(!normal('\u{FF08}', '\u{4F60}'));
        assert!(!normal('\u{4F60}', '\u{FF01}'));
        assert!(!normal('\u{4F60}', '\u{201D}'));
        assert!(normal('\u{201D}', '\u{4F60}'));
        assert!(normal('\u{4F60}', '\u{201C}'));
        assert!(!normal('\u{201C}', '\u{4F60}'));
    }

    #[test]
    fn cjk_latin_mixes_can_break() {
        assert!(normal('\u{4F60}', 'a'));
        assert!(normal('a', '\u{4F60}'));
        assert!(normal('\u{4E2D}', '\u{6587}'));
    }

    #[test]
    fn english_quotes_pair() {
        assert!(!normal('"', 'a'));
        assert!(!normal('a', '"'));
        assert!(!normal('"', '"'));
        assert!(!normal('\'', 'a'));
        assert!(!normal('a', '\''));
        assert!(!normal('\u{201C}', 'a'));
        assert!(!normal('a', '\u{201D}'));
        assert!(normal('\u{201D}', 'a'));
    }

    #[test]
    fn word_break_modes() {
        assert!(can_break('a', 'b', WordBreak::BreakAll));
        assert!(can_break('a', '\n', WordBreak::KeepAll));
        assert!(can_break(' ', 'a', WordBreak::KeepAll));
        assert!(!can_break('a', 'b', WordBreak::KeepAll));
        assert!(!can_break('\u{4F60}', '\u{597D}', WordBreak::KeepAll));
    }
}
