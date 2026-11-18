//! 中文/日文排版政策：成对标点挤压、行末标点悬挂、中西文自动间距。
//!
//! 三件事分开开关（[`crate::TextRules`]），零值全部关闭，与"零值 = 规范默认"
//! 一致。规则表照抄规范的可判定部分，并在每个函数上写出处：
//!
//! - **成对标点**：CSS Text Module Level 4 §8 `text-spacing-trim`；
//!   CLREQ（中文排版需求）§3.2「标点符号的宽度」；JLREQ §3.1「句読点・括弧類」。
//! - **行末悬挂**：CSS Text L4 §9 `hanging-punctuation: allow-end`；
//!   JLREQ §3.2「行末の句読点」。
//! - **自动间距**：CLREQ §3.3「中西文混排」建议的 1/4 em 空隙；不是 CSS 初始值。
//!
//! 与 `~/layx/render/punct.go` 的取舍一致：只挤压**可观测到**会被浏览器压掉的
//! 引号类标点，全角括号和顿号不压。浏览器是否压取决于字体是否支持，引擎不假装
//! 比浏览器多知道一层，所以表里只留 linecmp 量到的那几个字。

/// 标点在挤压规则里的角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PunctClass {
    None,
    /// 开引号 / 开括号：空白半边在左边。
    Open,
    /// 闭引号 / 闭括号：空白半边在右边。
    Close,
    /// 冒号、分号：空白半边在右边。
    Colon,
    /// 逗号、句号、顿号：空白半边在右边。
    Comma,
    /// 间隔号：两侧都是空白，本轮不挤压。
    MiddleDot,
    /// 全角空格：不参与挤压（它是真正的空白，由断行处理）。
    IdeographicSpace,
}

impl PunctClass {
    /// 空白半边是否在右边（闭标点、冒号、逗号）。
    fn trims_after(self) -> bool {
        matches!(
            self,
            PunctClass::Close | PunctClass::Colon | PunctClass::Comma
        )
    }
}

/// 标点分类。字符集来自 CSS Text L4 §8.2 的 `text-spacing-trim` 字符类，
/// 加上 U+2018/2019/201C/201D 四个中文对话实际使用的弯引号（规范列表里没有，
/// 但浏览器会压，见 `~/layx/render/punct.go` 的注释）。
pub(crate) fn class_of(ch: char) -> PunctClass {
    match ch {
        '\u{3008}' | '\u{300a}' | '\u{300c}' | '\u{300e}' | '\u{3010}' | '\u{3014}'
        | '\u{3016}' | '\u{3018}' | '\u{301a}' | '\u{301d}' | '\u{ff08}' | '\u{ff3b}'
        | '\u{ff5b}' | '\u{ff5f}' | '\u{ff62}' | '\u{2018}' | '\u{201c}' => PunctClass::Open,
        '\u{3009}' | '\u{300b}' | '\u{300d}' | '\u{300f}' | '\u{3011}' | '\u{3015}'
        | '\u{3017}' | '\u{3019}' | '\u{301b}' | '\u{301e}' | '\u{301f}' | '\u{ff09}'
        | '\u{ff3d}' | '\u{ff5d}' | '\u{ff60}' | '\u{ff63}' | '\u{2019}' | '\u{201d}' => {
            PunctClass::Close
        }
        '\u{ff1a}' | '\u{ff1b}' | '\u{fe13}' | '\u{fe14}' => PunctClass::Colon,
        '\u{3001}' | '\u{3002}' | '\u{ff0c}' | '\u{ff0e}' | '\u{ff64}' | '\u{fe10}'
        | '\u{fe11}' | '\u{fe12}' | '\u{fe50}' | '\u{fe51}' | '\u{fe52}' => PunctClass::Comma,
        '\u{30fb}' | '\u{ff65}' => PunctClass::MiddleDot,
        '\u{3000}' => PunctClass::IdeographicSpace,
        _ => PunctClass::None,
    }
}

/// 开标点里只有这些会真的被压掉左侧半宽（`~/layx/render/punct.go` 的
/// `trimmableOpening`，按 linecmp 的可观测结果固定）。
fn trimmable_opening(ch: char) -> bool {
    matches!(ch, '\u{2018}' | '\u{201c}' | '\u{3008}')
}

/// 闭标点里只有这些会真的被压掉右侧半宽。
fn trimmable_closing(ch: char) -> bool {
    matches!(ch, '\u{2019}' | '\u{201d}' | '\u{3009}')
}

/// 一对相邻标点里该压哪一个的半宽。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trim {
    /// 压左边字符的右半宽。
    Left,
    /// 压右边字符的左半宽。
    Right,
}

/// 判断 `text[a], text[b]`（`b == a + 1`）这对相邻字符要不要挤压。
///
/// 规则（CSS Text L4 §8.3 `text-spacing-trim: normal` 的可判定部分）：
///
/// - 两个全角标点相邻时，在它们之间压掉半宽；
/// - 半宽从"拥有空白半边"的那个字符上取：开标点给左半边，闭标点/冒号/逗号
///   给右半边；
/// - 开标点赢：一对里最多压半宽，开标点压不动时整对不压（它的左半边是挤压
///   唯一能来的地方）。
pub(crate) fn trim_target(left: char, right: char) -> Option<Trim> {
    let a = class_of(left);
    let b = class_of(right);
    if a == PunctClass::None || b == PunctClass::None {
        return None;
    }
    if b == PunctClass::Open {
        return trimmable_opening(right).then_some(Trim::Right);
    }
    if a.trims_after() && trimmable_closing(left) {
        return Some(Trim::Left);
    }
    None
}

/// 行末允许悬挂的闭标点集合：CJK 的 `。」」，；：？！` 与其余括号样式，
/// 见 CSS Text L4 §9 `hanging-punctuation: allow-end`。
pub(crate) fn is_hanging(ch: char) -> bool {
    matches!(
        ch,
        '\u{3001}'
            | '\u{3002}'
            | '\u{3009}'
            | '\u{300b}'
            | '\u{300d}'
            | '\u{300f}'
            | '\u{3011}'
            | '\u{3015}'
            | '\u{3017}'
            | '\u{3019}'
            | '\u{301b}'
            | '\u{301f}'
            | '\u{ff09}'
            | '\u{ff0c}'
            | '\u{ff0e}'
            | '\u{ff1a}'
            | '\u{ff1b}'
            | '\u{ff1f}'
            | '\u{ff01}'
            | '\u{ff3d}'
            | '\u{ff5d}'
            | '\u{ff60}'
            | '\u{2019}'
            | '\u{201d}'
    )
}

/// 行末悬挂可以吃掉的宽度：闭标点最多探出自身 advance 的一半。
///
/// 只影响"这一行放不放得下"和行盒右边界，不改 `Glyph.dx`。
pub(crate) fn hang_amount(ch: char, advance: f32) -> Option<f32> {
    if advance > 0.0 && is_hanging(ch) {
        Some(advance * 0.5)
    } else {
        None
    }
}

/// 表意文字（汉字、假名、谚文）：自动间距只加在它和拉丁字母/数字之间。
///
/// 范围取 Unicode 的 Ideographic / Hiragana / Katakana / Hangul 主区，
/// 不含全角标点（标点两侧不加，见 CLREQ §3.3）。
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
/// 只在相邻字符之间成立，所以不会出现在行首行尾（行首行尾没有"相邻"）。
pub(crate) fn needs_autospacing(left: char, right: char) -> bool {
    let latin_or_digit = |ch: char| ch.is_ascii_alphanumeric();
    (is_ideograph(left) && latin_or_digit(right)) || (latin_or_digit(left) && is_ideograph(right))
}
