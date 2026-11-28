//! 版式档案（typography profile）：**语言知识全部是数据**。
//!
//! 断行、分页、弹性调整这些**机制**不知道任何具体文字长什么样；它们只查这张表。
//! 新增一门语言 = 新增一个 `static`，不改 `flow.rs` / `paginate.rs` / `punct.rs`。
//!
//! 现在有两份：
//!
//! - [`GENERIC`]：零语言假设，等价于「浏览器默认」。所有 golden 与既有测试都跑它。
//! - [`ZH_HANS`]：简体中文（CLREQ）。只改表，不改机制。
//!
//! 选择由 `StyledDoc::meta.lang` 决定，元素级 `lang` 属性也已经在 IR 里，将来接
//! `:lang()` 时不用动这里。

use crate::punct::PunctClass;

/// 标点表。字符集合是**每门语言一份**，机制侧只做查表和几何运算。
///
/// 字段按「行内方向」描述（前侧/后侧），不按左右：竖排接进来时不用改语义。
pub(crate) struct PunctTable {
    /// 字符 → 标点类。必须按字符升序排列（测试会检查）。
    pub(crate) class: &'static [(char, PunctClass)],
    /// 成对挤压时允许吃掉**左**半宽的字符（开标点里字体真有半宽形的那些）。
    pub(crate) trim_open: &'static [char],
    /// 成对挤压时允许吃掉**右**半宽的字符。
    pub(crate) trim_close: &'static [char],
    /// 行末允许悬挂的字符。
    pub(crate) hang: &'static [char],
    /// 悬挂最多探出自身 advance 的多少（CLREQ / JLREQ 都是半个字）。
    pub(crate) hang_ratio: f32,
    /// 连续两个标点时不悬挂（CLREQ §6.1.3 末句）。
    pub(crate) hang_single_only: bool,
    /// 表意文字与拉丁字母/数字之间的默认空隙，单位 em；0 表示不加。
    pub(crate) autospace: f32,
    /// 落在行末时可以把前进宽度压到 `line_end_ratio` 的字符
    /// （CLREQ §6.3.2.3；GB/T 15834—2011 5.1.10）。
    pub(crate) line_end: &'static [char],
    /// 行末挤压后的宽度比例。0.5 = 半个字，中文出版惯例。
    pub(crate) line_end_ratio: f32,
}

/// 页尾出现孤行/寡行时的修复手段，按**优先级顺序**排在 [`PageRules::fixes`] 里。
///
/// 这是 CLREQ §7.1.2 那张方案清单的数据化：中文出版传统的第一选择是把孤行挪到
/// 上一页、"使其突出版心"（多占一行），而不是把内容往回撤、在页尾留一大片空白。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageFix {
    /// 让这一页多放一行，越过版心（`push_lines` 是上限）。
    PushOut,
    /// 把这一页的最后若干行挪到下一页。
    PullUp,
    /// 把整段撤到下一页。
    MoveWhole,
}

/// 怎么决定每一页从哪开始。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanKind {
    /// 排满一页，再回头试着修页尾的孤行：只看眼前一页。引入 profile 之前的
    /// 唯一做法，`generic` 必须保持它才能逐字节复现旧输出。
    Greedy,
    /// 整条单元流一次性挑切点，让"总代价"最小（页级动态规划）。
    Optimal,
}

/// 页级最优分页的代价权重。没有单位，只有相对大小有意义。
///
/// 这些数不是排版规则，是**取舍**：多留一行空白、让一行探出版心、把段落劈成
/// 两页——哪样更难看，只有把它们放在同一个标尺上才比得出来。中文的取舍写在
/// [`ZH_HANS`]，别的语言想换一套取舍只需要换这张表。
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageCost {
    /// 页底空掉一行空白 = `slack²`，平方是因为两页各空一行比一页空两行好看。
    pub(crate) slack: f32,
    /// 让这一页多放一行、探出版心：按 `over²` 计，另有一次性 `over_base`。
    pub(crate) over: f32,
    pub(crate) over_base: f32,
    /// 段落的最后一行孤零零落在下一页页首。
    pub(crate) widow: f32,
    /// 段落的第一行孤零零留在页尾。
    pub(crate) orphan: f32,
    /// 在段落中间断页（既不孤行也不寡行，只是劈开了）。
    pub(crate) split: f32,
    /// 违反了 `break-inside: avoid` 一类的要求。软约束，但很贵。
    pub(crate) avoid: f32,
}

impl PageCost {
    /// 中文正文的取舍（CLREQ §7.1.2）：宁可让一行探出版心，也不要孤行；
    /// 宁可劈开段落，也不要留一大片空白。
    ///
    /// 这些数是在《光之子》（2.9 万行、1441 页）上量出来的，不是猜的：
    /// `over` 定得比 `widow` 低，DP 才会优先用「多放一行」而不是「往回挪一行」
    /// 来消灭孤行——挪一行要在页底留一条空白，多放一行不用。`slack` 取平方，
    /// 因为十页各空一行比一页空十行好看。
    pub(crate) const ZH: PageCost = PageCost {
        slack: 1.0,
        over: 0.5,
        over_base: 0.5,
        widow: 8.0,
        orphan: 8.0,
        split: 0.2,
        avoid: 200.0,
    };

    /// 不做任何取舍（`Greedy` 用不到它）。
    pub(crate) const ZERO: PageCost = PageCost {
        slack: 0.0,
        over: 0.0,
        over_base: 0.0,
        widow: 0.0,
        orphan: 0.0,
        split: 0.0,
        avoid: 0.0,
    };
}

/// 行末余量怎么分配。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Justify {
    /// 有词距就全摊在词距上、没有就均摊字缝，每处一个值（浏览器默认的样子）。
    Flat,
    /// 先给西文词距（每处封顶半个字宽），剩下的才给字间隙（CLREQ §6.2.2.4）。
    /// 中西混排时这是区别所在：一行里同时有词距和字缝，只摊词距会把词距拉成坑。
    Tiered,
}

/// 怎么挑断点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinePlan {
    /// 从左往右，放不下就断。引入 profile 之前的唯一做法，`generic` 要保持它。
    Greedy,
    /// 一段的所有行一起挑断点（Knuth–Plass，行级动态规划）。
    Optimal,
}

/// 断行的代价表，只有 [`LinePlan::Optimal`] 用得到。
///
/// 和 [`PageCost`] 一样：这些数不是规则，是**取舍**。
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineCost {
    /// 断字（行末加连字符）一次的代价。
    pub(crate) hyphen: f32,
    /// 段落末行只剩一两个字的代价：宁可前面的行紧一点，也不要留孤字。
    pub(crate) orphan: f32,
}

impl LineCost {
    /// 中文的取舍：断字在中文里几乎没有（CJK 到处能断），孤字很扎眼。
    pub(crate) const ZH: LineCost = LineCost {
        hyphen: 50.0,
        orphan: 400.0,
    };

    pub(crate) const ZERO: LineCost = LineCost {
        hyphen: 0.0,
        orphan: 0.0,
    };
}

/// 行内断行与均排的规则。
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineRules {
    pub(crate) plan: LinePlan,
    pub(crate) justify: Justify,
    /// 每个字缝最多能收紧多少 em（CLREQ §6.2.2.3 的压缩侧）。
    ///
    /// 只在**需要挪一个位置**时用（孤字的那种「压行」），平时不动：
    /// 收紧是救急手段，常态是拉伸。
    pub(crate) gap_shrink: f32,
    /// 每个西文词距最多能收紧多少 em。
    pub(crate) space_shrink: f32,
    pub(crate) cost: LineCost,
}

pub(crate) struct PageRules {
    pub(crate) plan: PlanKind,
    /// `PlanKind::Optimal` 的取舍表；`Greedy` 用不到。
    pub(crate) cost: PageCost,
    /// 只对 `PlanKind::Greedy` 生效。`Optimal` 走 §16 的代价表，同一组取舍写在
    /// `cost` 里；这里留着是因为 DP 走到死角时会退回贪心，那时还得有话说。
    pub(crate) fixes: &'static [PageFix],
    /// `PageFix::PushOut` / DP 最多能越过版心几行。
    pub(crate) push_lines: u16,
    /// 把版心高度对齐到正文行高的整数倍（CLREQ §6.4 的纵横对齐前提）。
    ///
    /// 余数沉到页脚：每页行数固定，页底线永远在同一个位置。
    pub(crate) align_lines: bool,
    /// 孤字处理（CLREQ §7.1.2）：段落末行只剩一个字时，从上一行取一个字下来，
    /// 上一行重新均排。
    pub(crate) orphan_char: bool,
    /// 页底齐平（LaTeX `\flushbottom`）：页面高度除不尽行高时，把剩下的零头摊到
    /// 这一页的**段间距**上，而不是留一条白或者硬塞一行。
    ///
    /// 行距不动——行距是设计出来的，拉它等于改版式；段间距本来就有弹性。
    pub(crate) flush_bottom: bool,
}

pub(crate) struct Typography {
    /// 档案 id：进缓存键与诊断。
    #[allow(dead_code)]
    pub(crate) id: &'static str,
    pub(crate) punct: PunctTable,
    pub(crate) page: PageRules,
    pub(crate) line: LineRules,
    /// 块上没有显式对齐方式时用的对齐方式（覆盖 CSS 初始值 `start`）。
    ///
    /// 中文正文的常态是两端对齐（CLREQ §6.2.2「行内调整」之所以存在，
    /// 就是因为每行都要撑满版心）；浏览器默认的左对齐只是 web 的历史。
    /// `None` = 不改，保持 CSS 初始值。
    pub(crate) initial_align: Option<ebook_ir::TextAlign>,
}

// ---------------------------------------------------------------------------
// GENERIC：零语言假设。
//
// 除了 `§14` 的均排分配（分级拉伸把余量按词距→字间隙两档摊开，纯西文行仍按
// 整行均摊）之外，行为与引入 profile 之前逐字节一致；`pixel` 基线没动。
// ---------------------------------------------------------------------------

static GENERIC_CLASS: &[(char, PunctClass)] = &[
    ('\u{2018}', PunctClass::Open),
    ('\u{2019}', PunctClass::Close),
    ('\u{201c}', PunctClass::Open),
    ('\u{201d}', PunctClass::Close),
    ('\u{3000}', PunctClass::IdeographicSpace),
    ('\u{3001}', PunctClass::Comma),
    ('\u{3002}', PunctClass::Comma),
    ('\u{3008}', PunctClass::Open),
    ('\u{3009}', PunctClass::Close),
    ('\u{300a}', PunctClass::Open),
    ('\u{300b}', PunctClass::Close),
    ('\u{300c}', PunctClass::Open),
    ('\u{300d}', PunctClass::Close),
    ('\u{300e}', PunctClass::Open),
    ('\u{300f}', PunctClass::Close),
    ('\u{3010}', PunctClass::Open),
    ('\u{3011}', PunctClass::Close),
    ('\u{3014}', PunctClass::Open),
    ('\u{3015}', PunctClass::Close),
    ('\u{3016}', PunctClass::Open),
    ('\u{3017}', PunctClass::Close),
    ('\u{3018}', PunctClass::Open),
    ('\u{3019}', PunctClass::Close),
    ('\u{301a}', PunctClass::Open),
    ('\u{301b}', PunctClass::Close),
    ('\u{301d}', PunctClass::Open),
    ('\u{301e}', PunctClass::Close),
    ('\u{301f}', PunctClass::Close),
    ('\u{30fb}', PunctClass::MiddleDot),
    ('\u{fe10}', PunctClass::Comma),
    ('\u{fe11}', PunctClass::Comma),
    ('\u{fe12}', PunctClass::Comma),
    ('\u{fe13}', PunctClass::Colon),
    ('\u{fe14}', PunctClass::Colon),
    ('\u{fe50}', PunctClass::Comma),
    ('\u{fe51}', PunctClass::Comma),
    ('\u{fe52}', PunctClass::Comma),
    // ！和？是句末点号，规则上必须和「。」同类（CLREQ §6.1.3 的悬挂表里
    // 就有它们）。漏了这两个，`……呢！` 这种只有一个字的段末行就不会被认成孤字。
    ('\u{ff01}', PunctClass::Comma),
    ('\u{ff08}', PunctClass::Open),
    ('\u{ff09}', PunctClass::Close),
    ('\u{ff0c}', PunctClass::Comma),
    ('\u{ff0e}', PunctClass::Comma),
    ('\u{ff1a}', PunctClass::Colon),
    ('\u{ff1b}', PunctClass::Colon),
    ('\u{ff1f}', PunctClass::Comma),
    ('\u{ff3b}', PunctClass::Open),
    ('\u{ff3d}', PunctClass::Close),
    ('\u{ff5b}', PunctClass::Open),
    ('\u{ff5d}', PunctClass::Close),
    ('\u{ff5f}', PunctClass::Open),
    ('\u{ff60}', PunctClass::Close),
    ('\u{ff62}', PunctClass::Open),
    ('\u{ff63}', PunctClass::Close),
    ('\u{ff64}', PunctClass::Comma),
    ('\u{ff65}', PunctClass::MiddleDot),
];

pub(crate) static GENERIC: Typography = Typography {
    id: "generic",
    punct: PunctTable {
        class: GENERIC_CLASS,
        trim_open: &['\u{2018}', '\u{201c}', '\u{3008}'],
        trim_close: &['\u{2019}', '\u{201d}', '\u{3009}'],
        hang: &[
            '\u{2019}', '\u{201d}', '\u{3001}', '\u{3002}', '\u{3009}', '\u{300b}', '\u{300d}',
            '\u{300f}', '\u{3011}', '\u{3015}', '\u{3017}', '\u{3019}', '\u{301b}', '\u{301f}',
            '\u{ff01}', '\u{ff09}', '\u{ff0c}', '\u{ff0e}', '\u{ff1a}', '\u{ff1b}', '\u{ff1f}',
            '\u{ff3d}', '\u{ff5d}', '\u{ff60}',
        ],
        hang_ratio: 0.5,
        hang_single_only: false,
        autospace: 0.25,
        // 零语言假设的档案不挤压行末标点：闭括号压半宽会露出半个空格。
        line_end: &[],
        line_end_ratio: 0.5,
    },
    initial_align: None,
    // 断行、均排、压缩都和引入 profile 之前一样：行级 DP 与压行是中文档案的选择。
    line: LineRules {
        plan: LinePlan::Greedy,
        justify: Justify::Flat,
        gap_shrink: 0.0,
        space_shrink: 0.0,
        cost: LineCost::ZERO,
    },
    // 引入 profile 之前的分页器只有「往回退」和「整段撤走」两种手段，顺序也是
    // 退回优先。保持原样，既有 golden 与像素基线才不用动。
    page: PageRules {
        plan: PlanKind::Greedy,
        cost: PageCost::ZERO,
        fixes: &[PageFix::PullUp, PageFix::MoveWhole],
        push_lines: 0,
        align_lines: false,
        orphan_char: false,
        flush_bottom: false,
    },
};

// ---------------------------------------------------------------------------
// ZH_HANS：简体中文（CLREQ）
// ---------------------------------------------------------------------------

/// 简体中文的悬挂集合：只有**点号类**（顿号、逗号、句号、分号、冒号、问号、
/// 叹号）。CLREQ §6.1.3 明确不包含闭括号与闭引号——它们是全角字面居中，
/// 悬挂会露出半个空格。
static ZH_HANS_HANG: &[char] = &[
    '\u{3001}', '\u{3002}', '\u{ff01}', '\u{ff0c}', '\u{ff0e}', '\u{ff1a}', '\u{ff1b}', '\u{ff1f}',
];

pub(crate) static ZH_HANS: Typography = Typography {
    id: "zh-hans",
    punct: PunctTable {
        class: GENERIC_CLASS,
        trim_open: &['\u{2018}', '\u{201c}', '\u{3008}'],
        trim_close: &['\u{2019}', '\u{201d}', '\u{3009}'],
        hang: ZH_HANS_HANG,
        hang_ratio: 0.5,
        hang_single_only: true,
        autospace: 0.25,
        line_end: ZH_HANS_HANG,
        line_end_ratio: 0.5,
    },
    initial_align: Some(ebook_ir::TextAlign::Justify),
    // 行级 DP（Knuth–Plass）+ 压行：CLREQ §6.2.2.3 给压缩留了余地，§7.1.2 要求
    // 处理孤字。`gap_shrink` 取 0.05em：一行 30 来个字缝能收出一到一个半字，
    // 刚好够把孤字提上来，又看不出来（屏幕上 0.8px）。
    line: LineRules {
        plan: LinePlan::Optimal,
        justify: Justify::Tiered,
        gap_shrink: 0.05,
        space_shrink: 0.1,
        cost: LineCost::ZH,
    },
    // CLREQ §7.1.2：孤行首选「移至前一页，使其突出版心」；退不回去再往回撤；
    // 最后才整段换页。
    page: PageRules {
        plan: PlanKind::Optimal,
        cost: PageCost::ZH,
        fixes: &[PageFix::PushOut, PageFix::PullUp, PageFix::MoveWhole],
        push_lines: 1,
        align_lines: true,
        orphan_char: true,
        flush_bottom: true,
    },
};

/// 按 `meta.lang` / `lang` 属性选档案。
///
/// 认 `zh`、`zh-Hans`、`zh-CN`、`zh-SG` 及其大小写变体；`zh-Hant` / `zh-TW` /
/// `zh-HK` 暂时也落回 `ZH_HANS`（繁体的悬挂策略不同，等有测试用例再拆一份）。
pub(crate) fn of_lang(lang: Option<&str>) -> &'static Typography {
    let Some(lang) = lang else {
        return &GENERIC;
    };
    let mut buf = [0u8; 8];
    let head = {
        let bytes = lang.as_bytes();
        let n = bytes.len().min(buf.len());
        for (i, byte) in bytes[..n].iter().enumerate() {
            buf[i] = byte.to_ascii_lowercase();
        }
        &buf[..n]
    };
    let head = core::str::from_utf8(head).unwrap_or("");
    if head == "zh" || head.starts_with("zh-") {
        &ZH_HANS
    } else {
        &GENERIC
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(table: &[(char, PunctClass)]) {
        for pair in table.windows(2) {
            assert!(pair[0].0 < pair[1].0, "class 表必须按字符升序且无重复");
        }
    }

    #[test]
    fn class_table_is_sorted() {
        sorted(GENERIC_CLASS);
        assert_eq!(GENERIC_CLASS.len(), ZH_HANS.punct.class.len());
    }

    #[test]
    fn hang_tables_are_sorted() {
        for table in [GENERIC.punct.hang, ZH_HANS.punct.hang] {
            for pair in table.windows(2) {
                assert!(pair[0] < pair[1], "hang 表必须升序且无重复");
            }
        }
    }

    #[test]
    fn lang_selects_profile() {
        assert_eq!(of_lang(None).id, "generic");
        assert_eq!(of_lang(Some("en")).id, "generic");
        assert_eq!(of_lang(Some("zh")).id, "zh-hans");
        assert_eq!(of_lang(Some("zh-Hans")).id, "zh-hans");
        assert_eq!(of_lang(Some("ZH-CN")).id, "zh-hans");
        assert_eq!(of_lang(Some("zhuang")).id, "generic");
    }
}
