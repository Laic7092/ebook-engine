use ebook_ir::WordBreak;

use crate::measure::is_wide;

pub(crate) fn can_break(a: char, b: char, word_break: WordBreak) -> bool {
    if b == '\n' {
        return true;
    }
    if word_break == WordBreak::BreakAll {
        return true;
    }
    if a == ' ' {
        return true;
    }
    if b == ' ' {
        return false;
    }
    if is_wide(a) || is_wide(b) {
        return !no_start(b) && !no_end(a);
    }
    matches!(a, '-' | '/' | '\u{2013}' | '\u{2014}')
}

pub(crate) fn no_start(c: char) -> bool {
    matches!(
        c,
        '\u{3001}' | '\u{3002}' | '\u{FF0C}' | '\u{FF0E}' | '\u{FF1A}' | '\u{FF1B}' | '\u{FF1F}'
            | '\u{FF01}' | '\u{FF09}' | '\u{300D}' | '\u{300F}' | '\u{3011}' | '\u{300B}'
            | '\u{3009}' | '\u{201D}' | '\u{2019}' | '\u{2026}' | '\u{2014}' | '\u{00B7}'
            | '\u{FF05}' | '\u{FF3D}' | '\u{FF5D}'
    )
}

pub(crate) fn no_end(c: char) -> bool {
    matches!(
        c,
        '\u{FF08}' | '\u{300C}' | '\u{300E}' | '\u{3010}' | '\u{300A}' | '\u{3008}' | '\u{201C}'
            | '\u{2018}' | '\u{FF3B}' | '\u{FF5B}'
    )
}
