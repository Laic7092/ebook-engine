//! 版面质量度量——「尺子」。
//!
//! 这里不排版，只把 [`LayoutResult`] 里已经存在的几何读成可以互相比较的数字。
//! 目的很窄：改排版算法时，能说清「变好了」还是「只是测试没红」。
//!
//! 三条设计约束：
//!
//! - **不改结果**。只读 IR + [`TextMeasurer`]，没有字体也能跑（`StubMeasurer`
//!   下所有前进宽度是确定值），所以它和 golden 测试用同一套几何。
//! - **单位是人能读的**：缺口和拉伸以「字」（`em`）为单位，页底余量以「行」为
//!   单位。看到 `1.4` 就知道「空了一行多」。
//! - **不猜段落**。行盒里没有段落边界，靠 y 相邻去猜会被代码块、表格、引用骗到
//!   （一行 12 字、下一行 60 字，看起来像同一段）。所以「齐不齐」只看行的**右
//!   边缘**离版心右缘有多远，不分组。唯一还在猜段落的地方是孤字计数。
//! - **挂在 feature 后面**（`quality`，默认关闭）：wasm 宿主不用这份代码，就不该
//!   为它付体积。

use alloc::vec::Vec;

use ebook_ir::{LayoutResult, LineBox, Op};

use crate::measure::TextMeasurer;
use crate::shape::ShapeGlyph;

/// 一次排版的度量结果。所有分布都是**全篇**统计，不按页分组。
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub pages: usize,
    pub lines: usize,
    /// 版心宽度（各页取最大值）。
    pub measure: f32,
    /// 正文行高（出现最多的一档行盒高度）。
    pub line_h: f32,
    /// 版心右缘：行右缘里承载字形最多的那一档。
    pub edge: f32,
    /// 右缘落在版心右缘上的行数。
    ///
    /// 分母是**所有**行，含段末行和缩进的块：它们本来就该短。所以这个数是一
    /// 眼观感的**下界**，一本书里短段落越多它越低；横向比较（同一本书换档案、
    /// 或者改算法前后）才有意义。
    pub flush: usize,
    /// 右缘缩在版心以内的行数。
    pub short: usize,
    /// 右缘探出版心的行数（悬挂）。
    pub over: usize,
    /// 行末字形被压窄（标点挤压）的行数。
    pub squeezed: usize,
    /// 每行「版心右缘 − 本行右缘」，单位字。负值表示探出版心。
    ///
    /// 只在**两端对齐**的书上有意义。左对齐的书没有一条大家都在靠的边，
    /// `edge` 只是众数，缺口有正有负，均值会互相抵消。
    pub gap: Dist,
    /// 每行被拉开的量（`Σdx − Σ整形宽度`），单位字。
    pub stretch: Dist,
    /// 每页比「最常见的页行数」少几行，单位行（不含末页）。
    pub slack: Dist,
    /// 少排 ≥ 1 行的页数（不含末页）：书中间的「空洞页」。
    pub empty_tail: usize,
    /// 所有非末页一共少排了多少行。比「空洞页数」硬：页数一样多时，它衡量
    /// 这本书浪费了多少版心。
    pub wasted: usize,
    /// 突出版心的页数：内容越过版心最后一条格线（`Page.size.h` 因此长高的也算）。
    pub(crate) over_pages: usize,
    /// 跨页孤寡：下一页页首孤零零挂着上一段的最后一行（寡行），或者页尾孤零零
    /// 留着下一段的第一行（孤行）。按文本节点判断「同一段」。
    pub widows: usize,
    /// 段末孤字（末行只有 1 个字，或 1 个字 + 标点）的段落数。**估计值**。
    pub orphans: usize,
    /// 孤字落在哪些行（`RunMap.lines` 的全局下标）。
    ///
    /// 尺子自己记下来，诊断工具（`xtask orphans`）照着这个列，而不是拿同样
    /// 三条判据再实现一遍：两份实现迟早会分叉，然后就是「报表说还有 1 个，
    /// 工具说 0 个」这种最浪费时间的对话。
    pub orphan_lines: Vec<u32>,
    /// 每页行数。
    pub lines_per_page: Dist,
}

/// 一个分布的摘要。分位数用最近秩法，样本少时就是最大值。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dist {
    pub n: usize,
    pub mean: f32,
    pub sd: f32,
    pub p95: f32,
    pub max: f32,
}

impl Dist {
    pub fn of(samples: &mut [f32]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        samples.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let n = samples.len();
        let mean = samples.iter().sum::<f32>() / n as f32;
        let var = samples.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n as f32;
        // 最近秩：ceil(0.95n) 那个样本（从 1 数起），偏保守，不会低估尾部。
        let rank = (n * 95).div_ceil(100).max(1) - 1;
        Self {
            n,
            mean,
            sd: sqrt(var),
            p95: samples[rank],
            max: samples[n - 1],
        }
    }
}

/// 牛顿迭代开方：`core` 里没有 `f32::sqrt`（那是 `std`/`libm` 的），而为了
/// 一个标准差去引 `libm` 不值得。20 轮远超 f32 需要的精度，结果确定。
fn sqrt(value: f32) -> f32 {
    if value <= 0.0 {
        return 0.0;
    }
    let mut guess = value;
    for _ in 0..20 {
        guess = 0.5 * (guess + value / guess);
    }
    guess
}

/// 表头，和 [`Report`] 的 `Display` 对齐。
pub const HEADER: &str =
    "页   行  齐平  短行  悬挂  挤压   缺口p95   拉伸p95  空洞页  空行  孤行  孤字";

pub fn analyze(ir: &LayoutResult, measurer: &dyn TextMeasurer) -> Report {
    let measure = ir.pages.iter().map(|page| page.size.w).fold(0.0, f32::max);
    let line_h = body_line_height(ir).unwrap_or(0.0);
    let em = size_em(ir).max(0.001);
    let lines = &ir.runs.lines;

    // 版心右缘：全书行右缘里**承载字形最多**的那一档。
    //
    // 不能直接用页面内容右缘：真书的正文块几乎都带左右边距（`margin: 0 5pt`
    // 很常见），那 13px 的差足以把所有行都判成「没齐」。也不能用众数：一本
    // 插图册里窄栏行可能比正文行还多。按字形数加权，正文行每行几十个字、窄栏
    // 只几个字，加权后才反映「大多数字是靠哪条边排的」。
    let mut votes: Vec<(f32, usize)> = Vec::new();
    for line in lines {
        let edge = right_edge(line);
        match votes
            .iter_mut()
            .find(|(seen, _)| (*seen - edge).abs() < 0.5)
        {
            Some((_, count)) => *count += line.glyphs.len as usize,
            None => votes.push((edge, line.glyphs.len as usize)),
        }
    }
    let edge = votes
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(edge, _)| edge)
        .unwrap_or(measure);

    // 逐行累加自然宽度和实际宽度。行盒和字形 run 都按顺序排列，所以一个 run
    // 属于「起点不超过它的最后一行」。
    //
    // 自然宽度靠 `TextMeasurer::shape` 重新整形一遍算出来——IR 里的 `dx` 是
    // **拉伸之后**的值，只有和整形结果比才能说清「这一行被拉开了多少」。
    let mut natural = alloc::vec![0.0f32; lines.len()];
    let mut advance = alloc::vec![0.0f32; lines.len()];
    let mut squeezed = alloc::vec![false; lines.len()];
    let mut tail_char: Vec<Option<char>> = alloc::vec![None; lines.len()];
    let mut head_char: Vec<Option<char>> = alloc::vec![None; lines.len()];
    let mut shaped: Vec<ShapeGlyph> = Vec::new();
    for page in &ir.pages {
        for op in ir.display.op_slice(page.ops) {
            let Op::GlyphRun(run) = op else { continue };
            let Some(index) = line_of_glyph(lines, run.glyphs.start) else {
                continue;
            };
            shaped.clear();
            advance[index] += ir
                .display
                .glyph_slice(run.glyphs)
                .iter()
                .map(|glyph| glyph.dx)
                .sum::<f32>();
            natural[index] += measurer.shape(&run.text, run.font, run.size, &mut shaped);
            // 同一行后出现的 run 覆盖前者，循环结束时留下的是行末那个 run。
            squeezed[index] = last_glyph_is_squeezed(ir, run, &shaped, em);
            tail_char[index] = run.text.chars().last();
            // 一行的第一个 run 决定行首那个字；后面的 run 不覆盖。
            if head_char[index].is_none() {
                head_char[index] = run.text.chars().next();
            }
        }
    }

    let mut report = Report {
        pages: ir.pages.len(),
        lines: lines.len(),
        measure,
        line_h,
        edge,
        ..Report::default()
    };

    let mut gaps = Vec::with_capacity(lines.len());
    let mut stretches = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let gap = edge - right_edge(line);
        if gap < -0.5 {
            report.over += 1;
        } else if gap <= 1.0 {
            report.flush += 1;
        } else {
            report.short += 1;
        }
        if squeezed[index] {
            report.squeezed += 1;
        }
        if is_para_end(ir, index)
            && !is_single_line_para(ir, index)
            && is_orphan(line.glyphs.len as usize, head_char[index], tail_char[index])
        {
            report.orphans += 1;
            report.orphan_lines.push(index as u32);
        }
        gaps.push(gap / em);
        stretches.push((advance[index] - natural[index]) / em);
    }
    report.gap = Dist::of(&mut gaps);
    report.stretch = Dist::of(&mut stretches);

    // 页底余量：一页从最后一行底下到版心底，空了多少。
    //
    // 不按「比全书最常见的页行数少几行」量：那个众数会被「突出版心」和松紧不一的
    // 排版带跑，量出来的是噪声。绝对量只有一种读法，而且突出版心的页 `Page.size.h`
    // 跟着长高，自洽地算成 0——那正是我们希望的（它没有白留）。
    //
    // 跨页孤寡：同一文本节点跨过页边界，且这一侧只剩一行。
    for page in &ir.pages {
        let span = page.lines.as_range();
        let (Some(first), Some(last)) = (span.clone().next(), span.clone().last()) else {
            continue;
        };
        if first > 0 && lines[first].node == lines[first - 1].node && is_para_end(ir, first) {
            report.widows += 1;
        }
        if last + 1 < lines.len()
            && lines[last].node == lines[last + 1].node
            && last > 0
            && lines[last].node != lines[last - 1].node
        {
            report.widows += 1;
        }
    }

    // 末页不算：书排完了，最后一页短是常态。
    let unit = line_h.max(1.0);
    let body = ir.pages.len().saturating_sub(1);

    // 版心高度：取页盒高度的众数（突出版心的页把自己撑高了，不能参与）。
    let mut heights: Vec<f32> = ir.pages.iter().map(|page| page.size.h).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let nominal_h = heights[heights.len() / 2];
    let inset_h = ir
        .pages
        .first()
        .map(|page| page.inset.top + page.inset.bottom)
        .unwrap_or(0.0);
    let grid = (((nominal_h - inset_h).max(0.0)) / unit) as usize as f32 * unit;
    let mut counts: Vec<f32> = ir.pages[..body]
        .iter()
        .map(|page| ir.runs.lines[page.lines.as_range()].len() as f32)
        .collect();
    let mut slacks: Vec<f32> = Vec::with_capacity(body);
    let mut wasted = 0.0f32;
    for page in &ir.pages[..body] {
        let top = page.origin.y + page.inset.top;
        if ir.runs.lines[page.lines.as_range()]
            .last()
            .is_some_and(|line| line.rect.y + line.rect.h > top + grid + unit * 0.5)
        {
            report.over_pages += 1;
        }
        let bottom = page.origin.y + page.size.h - page.inset.bottom;
        let used = ir.runs.lines[page.lines.as_range()]
            .last()
            .map(|line| line.rect.y + line.rect.h)
            .unwrap_or(top);
        let slack = (bottom - used).max(0.0) / unit;
        wasted += slack;
        slacks.push(slack);
    }
    // no_std 没有 `f32::round`：正数加半个再截断就是四舍五入。
    report.wasted = (wasted + 0.5) as usize;
    report.empty_tail = slacks.iter().filter(|slack| **slack >= 0.5).count();
    report.slack = Dist::of(&mut slacks);
    report.lines_per_page = Dist::of(&mut counts);
    report
}

/// 行的右边缘。两端对齐看到的是它，不是行宽（首行缩进、引用的左右边距都会
/// 让行宽变窄，但右边缘仍然该齐）。
fn right_edge(line: &LineBox) -> f32 {
    line.rect.x + line.rect.w
}

/// 这一行是不是它所在段落的**唯一**一行。
///
/// 单独成行的「◇」「1」「……」是原文的一部分（场景分隔、编号），不是排版把
/// 孤字落下了。尺子不能把它们记在账上，否则永远修不完——而且会诱导后面的
/// 人去「修」一个本来就不该动的东西。
fn is_single_line_para(ir: &LayoutResult, index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let page = |line: usize| ir.page_of_line(line as u32).map(|page| page.index);
    page(index) != page(index - 1) || is_para_end(ir, index - 1)
}

/// 段末孤字：只剩 1 个字，或者 1 个字后面跟一个标点。
///
/// 用 `char::is_alphanumeric` 而不是内置标点表：尺子要能度量任何语言，不该
/// 依赖排版档案。
fn is_orphan(glyphs: usize, head: Option<char>, tail: Option<char>) -> bool {
    match (glyphs, head, tail) {
        // 一个字：一个字形、而且真的有字。没有文字的行（例如只有一个行内空元素）
        // 不是孤字——那是另一回事，别混进这个数。
        (1, Some(_), _) => true,
        // 两个字：前一个是字、后一个是标点。两个都是标点的（`……`、`——`）不算
        // 孤字——那是标点自己落到了一行，另一种毛病，不该混进这个数。
        (2, Some(first), Some(last)) => first.is_alphanumeric() && !last.is_alphanumeric(),
        _ => false,
    }
}

/// 字形下标落在哪一行：取「起点 ≤ 它」的最后一行。
fn line_of_glyph(lines: &[LineBox], glyph: u32) -> Option<usize> {
    let index = lines.partition_point(|line| line.glyphs.start <= glyph);
    index.checked_sub(1).filter(|i| *i < lines.len())
}

/// 行末字形是不是被压窄了（标点挤压）：整形出来的自然宽度 − IR 里的实际宽度。
fn last_glyph_is_squeezed(
    ir: &LayoutResult,
    run: &ebook_ir::GlyphRun,
    shaped: &[ShapeGlyph],
    em: f32,
) -> bool {
    let Some(last) = shaped.last() else {
        return false;
    };
    // 同一簇的字形共享 `ch`，把它们加起来才是这一簇的自然宽度。
    let cluster = shaped
        .iter()
        .rev()
        .take_while(|glyph| glyph.ch == last.ch)
        .map(|glyph| glyph.dx)
        .sum::<f32>();
    let Some(glyph) = ir.display.glyph_slice(run.glyphs).last() else {
        return false;
    };
    cluster - glyph.dx > em * 0.25
}

/// 正文字号（`em` 的基准）。
fn size_em(ir: &LayoutResult) -> f32 {
    body_em(ir).unwrap_or_else(|| body_line_height(ir).unwrap_or(1.0))
}

/// 本行是不是段落的最后一行（估计）。
///
/// 只给孤字计数用。判据：下一行不存在、换页、y 不相邻（块间距或表格换格）、
/// 左右边界同时变（嵌套块），或者换了**文本节点**且从该节点开头起。多文本
/// 节点的段落在中间换行时会被误判成段末，代价是孤字略多算。
/// 这一行是不是它所在段落的最后一行。
fn is_para_end(ir: &LayoutResult, index: usize) -> bool {
    let line = &ir.runs.lines[index];
    let Some(next) = ir.runs.lines.get(index + 1) else {
        return true;
    };
    if (next.rect.y - line.rect.bottom()).abs() > 0.5 {
        return true;
    }
    if (next.rect.x - line.rect.x).abs() > 0.5 && (right_edge(next) - right_edge(line)).abs() > 0.5
    {
        return true;
    }
    let page = |line: u32| ir.page_of_line(line).map(|page| page.index);
    if page(index as u32) != page(index as u32 + 1) {
        return true;
    }
    next.node != line.node && next.src.start == 0
}

/// 正文行高：行盒高度里出现次数最多的一档（标题、代码块不参与）。
fn body_line_height(ir: &LayoutResult) -> Option<f32> {
    let mut buckets: Vec<(f32, usize)> = Vec::new();
    for line in &ir.runs.lines {
        if line.rect.h <= 0.0 {
            continue;
        }
        bump(&mut buckets, line.rect.h);
    }
    buckets
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(h, _)| h)
}

/// 正文字号：字形 run 的尺寸里出现次数最多的一档。
fn body_em(ir: &LayoutResult) -> Option<f32> {
    let mut buckets: Vec<(f32, usize)> = Vec::new();
    for page in &ir.pages {
        for op in ir.display.op_slice(page.ops) {
            if let Op::GlyphRun(run) = op {
                if run.size > 0.0 {
                    bump(&mut buckets, run.size);
                }
            }
        }
    }
    buckets
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(s, _)| s)
}

fn bump(buckets: &mut Vec<(f32, usize)>, value: f32) {
    match buckets
        .iter_mut()
        .find(|(seen, _)| (*seen - value).abs() < 0.5)
    {
        Some((_, count)) => *count += 1,
        None => buckets.push((value, 1)),
    }
}

impl core::fmt::Display for Report {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let pct = |part: usize| {
            if self.lines == 0 {
                0.0
            } else {
                100.0 * part as f32 / self.lines as f32
            }
        };
        write!(
            f,
            "{:>3} {:>4} {:>5.0}% {:>5.0}% {:>4} {:>5} {:>9.2} {:>9.2} {:>6} {:>5} {:>5} {:>5} {:>5}",
            self.pages,
            self.lines,
            pct(self.flush),
            pct(self.short),
            self.over,
            self.squeezed,
            self.gap.p95,
            self.stretch.p95,
            self.empty_tail,
            self.wasted,
            self.over_pages,
            self.widows,
            self.orphans,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};
    use alloc::string::String;
    use alloc::vec;
    use ebook_ir::{EdgesF, StyledDoc};

    fn analyze_html(html: &str) -> Report {
        let doc: StyledDoc =
            ebook_format::xhtml::build(html, 0, &mut ebook_format::xhtml::NoAssets).expect("xhtml");
        let env = LayoutEnv {
            measurer: &StubMeasurer,
            resources: &NoImageSizes,
        };
        let constraint = PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0));
        let ir = layout(&doc, &env, &constraint);
        analyze(&ir, &StubMeasurer)
    }

    fn page(css: &str, body: &str, lang: &str) -> String {
        alloc::format!(
            "<html lang=\"{lang}\"><head><style>{css}</style></head><body>{body}</body></html>"
        )
    }

    fn 字(n: usize) -> String {
        core::iter::repeat_n('字', n).collect()
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let mut samples = (1..=20).map(|v| v as f32).collect::<Vec<_>>();
        let dist = Dist::of(&mut samples);
        assert_eq!(dist.n, 20);
        assert!((dist.mean - 10.5).abs() < 0.001);
        assert!((dist.p95 - 19.0).abs() < 0.001, "{dist:?}");
        assert!((dist.max - 20.0).abs() < 0.001);
        assert!((dist.sd - 5.766_963).abs() < 0.001, "{dist:?}");

        let mut one = vec![7.0];
        assert!((Dist::of(&mut one).p95 - 7.0).abs() < 0.001);
        assert_eq!(Dist::of(&mut Vec::new()).n, 0);
    }

    /// 两端对齐的中文段落：除段末行外每一行都该齐平，缺口为 0。
    #[test]
    fn a_justified_paragraph_has_no_ragged_lines() {
        let body = alloc::format!("<p>{}</p>", 字(200));
        let report = analyze_html(&page("p{margin:0}", &body, "zh"));
        assert_eq!(report.short, 1, "只有段末行是短的：{report}");
        assert_eq!(report.flush, report.lines - 1, "{report}");
        assert_eq!(report.over, 0, "{report}");
    }

    /// 左对齐的西文段落：按词断行，行尾参差，尺子必须看得出来。
    ///
    /// 中文不行：等宽字在贪心断行下每行都填到同样的字数，左对齐也不参差。
    #[test]
    fn a_ragged_paragraph_reports_short_lines() {
        let words = "typography is the craft of arranging type so that written ".repeat(6);
        let body = alloc::format!("<p>{words}</p>");
        let ragged = analyze_html(&page("p{margin:0;text-align:left}", &body, "en"));
        assert!(ragged.lines > 3, "{ragged}");

        // 同一段改成两端对齐，参差消失，只剩段末行。
        let justified = analyze_html(&page("p{margin:0;text-align:justify}", &body, "en"));
        assert_eq!(justified.short, 1, "{justified}");
        assert_eq!(justified.over, 0, "{justified}");

        // 尺子要能看出两者的差别，而不是靠绝对阈值：左对齐的短行更多，而且
        // 会有行探出「众数边」（说明根本不存在一条大家都在靠的边）。
        assert!(ragged.short > justified.short, "{ragged} vs {justified}");
        assert!(ragged.over > justified.over, "{ragged} vs {justified}");
    }

    /// 「……」落单不算孤字：两个都是标点，那是标点自己落到了一行，另一种毛病。
    /// 单独成段的一个字（「◇」、编号「1」）也不是排版弄出来的。
    ///
    /// 两种都不该记在孤字账上：记了永远修不完，而且会引导后面的人去「修」一个
    /// 本来就不该动的东西。
    #[test]
    fn the_ruler_ignores_what_was_alone_in_the_source() {
        let css = "p{margin:0;line-height:24px;font-size:10px}";
        let env = LayoutEnv {
            measurer: &StubMeasurer,
            resources: &NoImageSizes,
        };
        // 版心 100px：每行 10 个字。
        let constraint = PageConstraint::new(148.0, 800.0).with_margin(EdgesF::all(24.0));
        let run = |lang: &str, body: &str| {
            let html = page(css, body, lang);
            let doc: StyledDoc =
                ebook_format::xhtml::build(&html, 0, &mut ebook_format::xhtml::NoAssets)
                    .expect("xhtml");
            analyze(&layout(&doc, &env, &constraint), &StubMeasurer)
        };

        // 20 个字正好两行，剩下「……」独占第三行。
        let report = run("zh", &alloc::format!("<p>{}……</p>", 字(20)));
        assert_eq!(report.lines, 3, "{report}");
        assert_eq!(report.orphans, 0, "{report}");

        // 一整段就一个字：原文如此。
        assert_eq!(run("zh", "<p>◇</p>").orphans, 0);

        // 对照：真正被排版挤下来的孤字仍然要记账（generic 不做孤字修复）。
        let report = run("en", &alloc::format!("<p>{}甲</p>", 字(20)));
        assert_eq!(report.lines, 3, "{report}");
        assert_eq!(report.orphans, 1, "{report}");
    }

    /// 孤字修复前后：同一个文档，档案决定末端孤字有没有。
    #[test]
    fn the_ruler_sees_orphans_that_the_profile_fixes() {
        // 21 个字、每行 10 个：generic 排成 10/10/1，zh 把最后一个字拉回去。
        let body = alloc::format!("<p>{}</p>", 字(21));
        let css = "p{margin:0;line-height:24px;font-size:10px}";
        let html = page(css, &body, "zh");
        let doc: StyledDoc =
            ebook_format::xhtml::build(&html, 0, &mut ebook_format::xhtml::NoAssets)
                .expect("xhtml");
        let env = LayoutEnv {
            measurer: &StubMeasurer,
            resources: &NoImageSizes,
        };
        // 版心 100px：每行 10 个字。
        let constraint = PageConstraint::new(148.0, 800.0).with_margin(EdgesF::all(24.0));
        let ir = layout(&doc, &env, &constraint);
        let report = analyze(&ir, &StubMeasurer);
        assert_eq!(report.lines, 3, "{report}");
        // zh 档案把最后一个字拉回上一行，末行有 2 个字，不算孤字。
        assert_eq!(report.orphans, 0, "{report}");

        let mut doc = doc;
        doc.meta.lang = None;
        let ir = layout(&doc, &env, &constraint);
        let report = analyze(&ir, &StubMeasurer);
        assert_eq!(report.orphans, 1, "{report}");
    }

    /// 标点挤压：行末的「。」被压到半宽，行就齐了。
    #[test]
    fn squeezed_line_ends_are_counted() {
        let body = "<p>一二三四五六七八九。</p>";
        let css = "p{margin:0;font-size:10px;line-height:24px}";
        let html = page(css, body, "zh");
        let doc: StyledDoc =
            ebook_format::xhtml::build(&html, 0, &mut ebook_format::xhtml::NoAssets)
                .expect("xhtml");
        let env = LayoutEnv {
            measurer: &StubMeasurer,
            resources: &NoImageSizes,
        };
        // 版心 95px：十个全角字放不下，挤压「。」后刚好一行。
        let constraint = PageConstraint::new(143.0, 800.0).with_margin(EdgesF::all(24.0));
        let ir = layout(&doc, &env, &constraint);
        let report = analyze(&ir, &StubMeasurer);
        assert_eq!(report.lines, 1, "{report}");
        assert_eq!(report.squeezed, 1, "{report}");
    }
}
