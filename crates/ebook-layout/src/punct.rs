//! 标点规则的**机制**：分类、成对挤压、行末悬挂、自动间距。
//!
//! 这里没有任何具体文字。字符集合、悬挂比例、自动间距宽度全部来自
//! [`Typography::punct`](crate::typography::Typography)。新增一门语言只改
//! `typography.rs` 里的一张表，不碰这个文件。
//!
//! 每个函数都接受 `&PunctTable`，所以同一个机制可以同时服务多门语言。

use crate::typography::PunctTable;

/// 标点在挤压规则里的角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PunctClass {
    None,
    /// 开引号 / 开括号：空白半边在行内方向的前侧。
    Open,
    /// 闭引号 / 闭括号：空白半边在后侧。
    Close,
    /// 冒号、分号：空白半边在后侧。
    Colon,
    /// 逗号、句号、顿号：空白半边在后侧。
    Comma,
    /// 间隔号：两侧都是空白。
    MiddleDot,
    /// 全角空格：不参与挤压（它是真正的空白，由断行处理）。
    IdeographicSpace,
}

impl PunctClass {
    /// 空白半边是否在后侧（闭标点、冒号、逗号）。
    fn trims_after(self) -> bool {
        matches!(
            self,
            PunctClass::Close | PunctClass::Colon | PunctClass::Comma
        )
    }
}

/// 查表分类。表按字符升序，查找是对数时间。
pub(crate) fn class_of(table: &PunctTable, ch: char) -> PunctClass {
    match table.class.binary_search_by_key(&ch, |&(c, _)| c) {
        Ok(index) => table.class[index].1,
        Err(_) => PunctClass::None,
    }
}

fn contains(list: &[char], ch: char) -> bool {
    list.binary_search(&ch).is_ok()
}

/// 一对相邻标点里该压哪一个的半宽。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trim {
    /// 压左边字符的后半宽。
    Left,
    /// 压右边字符的前半宽。
    Right,
}

/// 判断 `text[a], text[b]`（`b == a + 1`）这对相邻字符要不要挤压。
///
/// 规则（CSS Text L4 §8.3 `text-spacing-trim: normal` 的可判定部分）：
///
/// - 两个全角标点相邻时，在它们之间压掉半宽；
/// - 半宽从「拥有空白半边」的那个字符上取：开标点取前侧，闭标点/冒号/逗号
///   取后侧；
/// - 开标点赢：一对里最多压半宽，开标点压不动时整对不压（它的前侧半边是
///   挤压唯一能来的地方）。
pub(crate) fn trim_target(table: &PunctTable, left: char, right: char) -> Option<Trim> {
    let a = class_of(table, left);
    let b = class_of(table, right);
    if a == PunctClass::None || b == PunctClass::None {
        return None;
    }
    if b == PunctClass::Open {
        return contains(table.trim_open, right).then_some(Trim::Right);
    }
    if a.trims_after() && contains(table.trim_close, left) {
        return Some(Trim::Left);
    }
    None
}

/// 行末悬挂可以吃掉的宽度：闭标点最多探出自身 advance 的 `hang_ratio`。
///
/// 只影响「这一行放不放得下」和行盒右边界，不改 `Glyph.dx`。
pub(crate) fn hang_amount(table: &PunctTable, ch: char, advance: f32) -> Option<f32> {
    if advance > 0.0 && contains(table.hang, ch) {
        Some(advance * table.hang_ratio)
    } else {
        None
    }
}

/// 行末标点挤压：这个字符落在行末时，前进宽度可以压到多少。
///
/// 返回 `None` 表示这个字符不参与行末挤压（闭括号、闭引号压半宽会露出半个
/// 空格，所以它们不在表里）。CLREQ §6.3.2.3 / GB/T 15834—2011 5.1.10：
/// 全角标点排在一行末尾时占半角字符的宽度。
pub(crate) fn line_end_fit(table: &PunctTable, ch: char, advance: f32) -> Option<f32> {
    if advance <= 0.0 || !contains(table.line_end, ch) {
        return None;
    }
    Some(advance * table.line_end_ratio)
}

/// 表意文字（汉字、假名、谚文）。这是 Unicode 属性，不是某门语言的规则。
///
/// 自动间距只加在它和拉丁字母/数字之间。
pub(crate) fn is_ideograph(ch: char) -> bool {
    let c = ch as u32;
    matches!(
        c,
        0x1100..=0x11FF
            | 0x3040..=0x30FF
            | 0x3130..=0x318F
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0x20000..=0x3FFFD
    )
}

/// 中西文之间的条件空隙：一侧表意、另一侧是拉丁字母或数字。
///
/// 只在相邻字符之间成立，所以不会出现在行首行尾（行首行尾没有「相邻」）。
/// 具体宽度由档案给（0 表示这门语言不加）。
pub(crate) fn needs_autospacing(left: char, right: char) -> bool {
    let latin_or_digit = |ch: char| ch.is_ascii_alphanumeric();
    (is_ideograph(left) && latin_or_digit(right)) || (latin_or_digit(left) && is_ideograph(right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typography::{GENERIC, ZH_HANS};

    #[test]
    fn classification_is_language_independent_of_mechanism() {
        let t = &GENERIC.punct;
        assert_eq!(class_of(t, '\u{300c}'), PunctClass::Open);
        assert_eq!(class_of(t, '\u{300d}'), PunctClass::Close);
        assert_eq!(class_of(t, '\u{ff0c}'), PunctClass::Comma);
        assert_eq!(class_of(t, '\u{3000}'), PunctClass::IdeographicSpace);
        assert_eq!(class_of(t, 'a'), PunctClass::None);
        // ！和？是句末点号，和「，」「。」同一类。漏了它们，末行「呢！」就不会
        // 被认成孤字——孤字判定查的正是这张表。
        assert_eq!(class_of(t, '\u{ff01}'), PunctClass::Comma);
        assert_eq!(class_of(t, '\u{ff1f}'), PunctClass::Comma);
    }

    #[test]
    fn trim_pairs() {
        for table in [&GENERIC.punct, &ZH_HANS.punct] {
            assert_eq!(
                trim_target(table, '\u{300c}', '\u{201c}'),
                Some(Trim::Right)
            );
            assert_eq!(trim_target(table, '\u{201d}', '\u{300d}'), Some(Trim::Left));
            assert_eq!(trim_target(table, '\u{300c}', '\u{300d}'), None);
            assert_eq!(trim_target(table, 'a', '\u{300c}'), None);
        }
    }

    #[test]
    fn hanging_sets_differ_by_profile() {
        // 闭括号在通用档案里可以悬挂，简体中文档案里不行（CLREQ §6.1.3）。
        assert!(hang_amount(&GENERIC.punct, '\u{300d}', 10.0).is_some());
        assert!(hang_amount(&ZH_HANS.punct, '\u{300d}', 10.0).is_none());
        assert!(hang_amount(&ZH_HANS.punct, '\u{3002}', 10.0).is_some());
    }
}
