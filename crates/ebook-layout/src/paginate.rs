//! 分页：在第一遍产出的扁平单元流上，决定每一页从哪开始。
//!
//! 这一层**只关心高度**。它不知道文字、语言、标点；页尾出现孤行/寡行时用哪几种
//! 修复手段、按什么顺序试，全部来自 [`PageRules`](crate::typography::PageRules)。

use alloc::vec::Vec;
use ebook_ir::{EdgesF, Op, Page, PointF, SizeF};

use crate::typography::{PageCost, PageFix, PageRules, PlanKind};

#[derive(Clone, Copy)]
pub(crate) struct Unit {
    pub(crate) y: f32,
    pub(crate) h: f32,
    pub(crate) line: bool,
    pub(crate) block: u32,
    pub(crate) order: u16,
    pub(crate) len: u16,
    pub(crate) orphans: u16,
    pub(crate) widows: u16,
    pub(crate) force: bool,
    pub(crate) avoid_before: bool,
    pub(crate) avoid_after: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PageStart {
    pub(crate) unit: u32,
    pub(crate) y: f32,
    /// 这一页越过版心的高度（`PageFix::PushOut`）。0 表示正好齐底。
    pub(crate) extra: f32,
}

/// 分页计划：页起点 + 是否因 `max_pages` 截断。
///
/// `cut` 是第一个“本来会开新页”的 unit 下标：`0..cut` 的内容已经被 `starts`
/// 覆盖，`cut..` 的内容应当整体丢弃，才能保证每一行都落在自己的页高内。
pub(crate) struct Plan {
    pub(crate) starts: Vec<PageStart>,
    pub(crate) truncated: bool,
    pub(crate) cut: Option<usize>,
}

pub(crate) fn plan(
    units: &[Unit],
    fit_h: f32,
    box_h: f32,
    line_h: f32,
    max_pages: usize,
    rules: &PageRules,
) -> Plan {
    match rules.plan {
        PlanKind::Greedy => plan_greedy(units, fit_h, box_h, max_pages, rules),
        PlanKind::Optimal => plan_optimal(units, fit_h, box_h, line_h, max_pages, rules),
    }
}

fn plan_greedy(
    units: &[Unit],
    fit_h: f32,
    box_h: f32,
    max_pages: usize,
    rules: &PageRules,
) -> Plan {
    let cap = max_pages.max(1);
    // `fit_h` 是“真正用来放行的高度”（可能已被对齐到行高网格），
    // `box_h` 是页盒里内容区的高度。两者之差就是沉到页脚的余数。
    let h = fit_h.max(1.0);

    // 分页前先用内容高度估页数上界，只用来给 `starts` 定容量：
    // 真正的截断发生在下面“需要开新页但已经到 max_pages”时。
    let total_h = units.last().map(|unit| unit.y + unit.h).unwrap_or(0.0);
    // no_std 下没有 f32::ceil：正数截断取整再 +2 就是保守上界。
    let estimate = if total_h <= 0.0 {
        1
    } else {
        (total_h / h) as usize + 2
    };

    let mut starts = Vec::with_capacity(estimate.min(cap));
    starts.push(PageStart {
        unit: 0,
        y: 0.0,
        extra: 0.0,
    });
    let n = units.len();
    if n == 0 {
        return Plan {
            starts,
            truncated: false,
            cut: None,
        };
    }
    let mut s = 0usize;
    let mut top = 0.0f32;

    while s < n {
        let limit = top + h + 0.001;
        let mut e = s;
        while e < n {
            if e > s && units[e].force {
                break;
            }
            if units[e].y + units[e].h > limit {
                if e == s {
                    e += 1;
                }
                break;
            }
            e += 1;
        }
        if e >= n {
            break;
        }

        let (cut, extra) = resolve(units, s, e, rules, top, h, box_h);
        // `extra` 属于**正在结束的这页**（从 `s` 开始的页），不是新开的下一页。
        if let Some(last) = starts.last_mut() {
            last.extra = extra;
        }
        let cut = cut.clamp(s + 1, n);
        if cut >= n {
            break;
        }
        // 当前页已经是第 max_pages 页，再往下就要越预算：整体丢弃 cut..。
        if starts.len() >= cap {
            return Plan {
                starts,
                truncated: true,
                cut: Some(cut),
            };
        }
        top = units[cut].y.max(top + 1.0);
        starts.push(PageStart {
            unit: cut as u32,
            y: top,
            extra,
        });
        s = cut;
    }

    Plan {
        starts,
        truncated: false,
        cut: None,
    }
}

/// 页级动态规划：一次挑完整本书的切点。
///
/// 贪心分页是「排满一页，再回头看页尾丑不丑」——它只看得到一页，所以每修一次
/// 页尾，空洞就往下推一页，越改越乱。这里换个问法：把「单元 `s..e` 算一页」看成
/// 一条边、边上记这页的代价，然后求 `0` 到 `n` 的最短路径。路径一次定完，
/// 不存在「前面挪了半格、后面翻面」。
///
/// 单元数上万，但一页只装得下几十个单元，所以每个起点只往后看一个窗口，
/// 总代价 O(n × 每页单元数)。
fn plan_optimal(
    units: &[Unit],
    fit_h: f32,
    box_h: f32,
    line_h: f32,
    max_pages: usize,
    rules: &PageRules,
) -> Plan {
    let cap = max_pages.max(1);
    let h = fit_h.max(1.0);
    let n = units.len();
    if n == 0 {
        return Plan {
            starts: alloc::vec![PageStart {
                unit: 0,
                y: 0.0,
                extra: 0.0
            }],
            truncated: false,
            cut: None,
        };
    }

    // 突出版心最多几行：和贪心一样由档案给。行高取**正文**的（文档级），
    // 不取这一页最后一个单元——否则页尾放一张大图会把「一行」撑成 100px。
    let body = line_h.max(1.0);
    let push_band = rules.push_lines as f32;
    let mut best = alloc::vec![f32::INFINITY; n + 1];
    let mut prev = alloc::vec![u32::MAX; n + 1];
    best[0] = 0.0;

    for s in 0..n {
        if best[s] == f32::INFINITY {
            continue;
        }
        let top = units[s].y;
        let mut e = s;
        while e < n {
            // 强制分页：这一页到此为止，`e` 是下一页的开头。
            if e > s && units[e].force {
                break;
            }
            e += 1;
            let bottom = units[e - 1].y + units[e - 1].h;
            let used = bottom - top;
            if used > h + body * push_band + 0.001 && e > s + 1 {
                break;
            }
            // 全书最后一页短是常态，不罚。
            let cost = if e == n {
                best[s]
            } else {
                best[s] + page_cost(units, e, used, h, body, rules.cost)
            };
            if cost < best[e] {
                best[e] = cost;
                prev[e] = s as u32;
            }
        }
    }

    if best[n] == f32::INFINITY {
        // 一页一个单元总能走下去；真走到这里说明单元流本身有问题，
        // 退回贪心至少能出一份结果。
        return plan_greedy(units, fit_h, box_h, max_pages, rules);
    }

    let mut ends = alloc::vec::Vec::new();
    let mut at = n;
    while at > 0 {
        ends.push(at);
        at = prev[at] as usize;
    }
    ends.reverse();

    let mut starts = alloc::vec::Vec::with_capacity(ends.len());
    let mut begin = 0usize;
    let mut top = 0.0f32;
    for &end in &ends {
        let bottom = units[end - 1].y + units[end - 1].h;
        // 页盒只在真的越过内容区时才长高；落在网格余数里的那一截本来就没用上。
        let extra = (bottom - (top + box_h)).max(0.0);
        starts.push(PageStart {
            unit: begin as u32,
            y: top,
            extra,
        });
        begin = end;
        if end < n {
            top = units[end].y;
        }
    }

    if starts.len() > cap {
        let cut = starts[cap].unit as usize;
        starts.truncate(cap);
        return Plan {
            starts,
            truncated: true,
            cut: Some(cut),
        };
    }
    Plan {
        starts,
        truncated: false,
        cut: None,
    }
}

/// 一页（单元 `s..e`）的代价。数值没有单位，只有相对大小有意义。
fn page_cost(
    units: &[Unit],
    e: usize,
    used: f32,
    fit_h: f32,
    line_h: f32,
    weight: PageCost,
) -> f32 {
    let slack = (fit_h - used) / line_h;
    let mut total = if slack >= 0.0 {
        weight.slack * slack * slack
    } else {
        weight.over_base + weight.over * slack * slack
    };
    let prev = units[e - 1];
    let next = units[e];
    if (prev.line && prev.avoid_after) || (next.line && next.avoid_before) {
        total += weight.avoid;
    }
    if prev.line && next.line && prev.block == next.block && prev.len > 0 {
        let total_lines = prev.len as usize;
        let before = prev.order as usize + 1;
        if before <= total_lines {
            let after = total_lines - before;
            let orphans = prev.orphans.max(1) as usize;
            let widows = prev.widows.max(1) as usize;
            if after < widows {
                total += weight.widow;
            } else if before < orphans {
                total += weight.orphan;
            } else {
                total += weight.split;
            }
        }
    }
    total
}

/// 滚动模式的计划：整章只有一页，从 unit 0、y 0 开始。
///
/// 断行结果和分页模式完全一样，这里只是不切页；`Page.size.h` 由调用方
/// 换成内容实际高度。
pub(crate) fn plan_scroll() -> Plan {
    Plan {
        starts: alloc::vec![PageStart {
            unit: 0,
            y: 0.0,
            extra: 0.0
        }],
        truncated: false,
        cut: None,
    }
}

/// 先按 `break-before/after`（必须遵守）把断点往回退，再按档案给的**优先级**
/// 依次尝试孤行/寡行的修复手段。
fn resolve(
    units: &[Unit],
    s: usize,
    greedy: usize,
    rules: &PageRules,
    top: f32,
    fit_h: f32,
    box_h: f32,
) -> (usize, f32) {
    let mut cut = greedy;
    while cut > s + 1 {
        let next = step(units, s, cut);
        if next >= cut {
            break;
        }
        cut = next.max(s + 1);
    }
    if cut >= units.len() {
        return (cut, 0.0);
    }
    match orphan_widow_fix(units, s, cut, rules, top, fit_h, box_h) {
        Some(fixed) => fixed,
        None => (cut, 0.0),
    }
}

fn step(units: &[Unit], s: usize, cut: usize) -> usize {
    let prev = units[cut - 1];
    let next = units[cut];
    if (prev.line && prev.avoid_after) || (next.line && next.avoid_before) {
        return (cut - 1).max(s + 1);
    }
    cut
}

/// 跨页的同一段落：`cut` 落在段落内部时的孤寡修复。
///
/// 返回 `(新的断点, 该页越过版心的高度)`；`None` 表示没有可用手段，保持贪心断点。
fn orphan_widow_fix(
    units: &[Unit],
    s: usize,
    cut: usize,
    rules: &PageRules,
    top: f32,
    fit_h: f32,
    box_h: f32,
) -> Option<(usize, f32)> {
    let prev = units[cut - 1];
    let next = units[cut];
    if !prev.line || !next.line || prev.block != next.block || prev.len == 0 {
        return None;
    }
    let total = prev.len as usize;
    let before = prev.order as usize + 1;
    if before > total {
        return None;
    }
    let after = total - before;
    let orphans = prev.orphans.max(1) as usize;
    let widows = prev.widows.max(1) as usize;
    if before >= orphans && after >= widows {
        return None;
    }
    let line_h = prev.h.max(1.0);
    let first = block_first(units, s, cut, prev.block);

    for fix in rules.fixes {
        match *fix {
            PageFix::PushOut => {
                // 把剩下的几行留在一页里，页底线越出版心。CLREQ §7.1.2 的
                // 第一选择：宁可探出一点，也不留一片空白。
                if after == 0 || after > rules.push_lines as usize {
                    continue;
                }
                let end = cut + after;
                if end > units.len() {
                    continue;
                }
                // 不跨过强制断点（下一章/下一节必须新起一页时不能硬塞）。
                if units[cut..end].iter().skip(1).any(|unit| unit.force) {
                    continue;
                }
                let bottom = units[end - 1].y + units[end - 1].h;
                let over_fit = bottom - (top + fit_h);
                if over_fit <= line_h * rules.push_lines as f32 + 0.001 {
                    // 页盒只在**真的越过内容区**时才长高；网格余数内的部分
                    // 本来就没被用上，不需要改页盒。
                    let over_box = bottom - (top + box_h);
                    return Some((end, over_box.max(0.0)));
                }
            }
            PageFix::PullUp => {
                if after >= widows {
                    continue;
                }
                let need = widows - after;
                let keep = before.checked_sub(need)?;
                if keep < orphans {
                    continue;
                }
                let moved = back_lines(units, s, cut, need);
                if moved > s {
                    return Some((moved, 0.0));
                }
            }
            PageFix::MoveWhole => {
                if before < orphans && first > s {
                    return Some((first, 0.0));
                }
            }
        }
    }
    None
}

fn block_first(units: &[Unit], s: usize, cut: usize, block: u32) -> usize {
    let mut first = cut;
    while first > s && units[first - 1].line && units[first - 1].block == block {
        first -= 1;
    }
    first
}

fn back_lines(units: &[Unit], s: usize, cut: usize, count: usize) -> usize {
    let mut c = cut;
    let mut left = count;
    while c > s + 1 && left > 0 {
        c -= 1;
        if units[c].line {
            left -= 1;
        }
    }
    c
}

pub(crate) fn page_of(starts: &[PageStart], index: usize) -> u32 {
    let found = starts.partition_point(|s| s.unit as usize <= index);
    (found.max(1) - 1) as u32
}

/// 页底齐平（LaTeX `\flushbottom`）：把每页除不尽的零头摊到页内的**空隙**上。
///
/// 返回每页的拉伸比例（`0` = 不拉伸），组装侧按「每个空隙 × 比例」把后面的行
/// 往下推。只摊到真正的空隙（段间距、块间距），不动行距：行距是设计出来的，
/// 拉它等于改版式；段间距本来就有弹性。
///
/// 两道保险：一处空隙都不够（只有一道缝）时不拉伸，否则那一整块零头会变成一条
/// 沟；每道缝最多撑开半行。摊不完就摊不完，页底略不齐也比一条沟强。
/// 每道缝最多撑开多少个行高（见 [`flush_scale`]）。
const SLACK_PER_GAP: f32 = 0.1;

pub(crate) fn flush_scale(
    units: &[Unit],
    starts: &[PageStart],
    fit_h: f32,
    line_h: f32,
) -> Vec<f32> {
    let mut scales = alloc::vec![0.0f32; starts.len()];
    let unit = line_h.max(1.0);
    for (index, start) in starts.iter().enumerate() {
        // 末页短是书排完了，不是缺陷；它不该被撑满。
        if index + 1 == starts.len() {
            break;
        }
        let s = start.unit as usize;
        let e = starts
            .get(index + 1)
            .map(|next| next.unit as usize)
            .unwrap_or(units.len());
        if e <= s {
            continue;
        }
        let used = units[e - 1].y + units[e - 1].h - start.y;
        let slack = fit_h - used;
        if slack <= 0.5 {
            continue;
        }
        let mut gaps = 0.0f32;
        let mut count = 0usize;
        for i in (s + 1)..e {
            let gap = units[i].y - (units[i - 1].y + units[i - 1].h);
            if gap > 0.5 {
                gaps += gap;
                count += 1;
            }
        }
        if count < 2 || gaps <= 0.5 {
            continue;
        }
        // 每道缝最多撑 `SLACK_PER_GAP` 个行高。这个数不是审美，是**观感**：
        // 一段 10px 的段间距撑到 13px 看不出来，撑到 26px 就成了「这页怎么这么松」。
        // 撑不下就别撑——页底留一条白，比把整页的段间距改掉更像书。
        let stretch = slack.min(count as f32 * unit * SLACK_PER_GAP);
        scales[index] = stretch / gaps;
    }
    scales
}

/// 把版心高度对齐到正文行高的整数倍。
///
/// 行高取**行数最多的那一档**（正文），标题/代码块这种少数派不影响版心。
/// 余数沉到页脚：这是纸书「版心 = N 行」的做法，也让页底线永远不动。
/// 返回 `None` 表示无法对齐（没有行、行高非正、一页放不下一行），调用方
/// 保持原始版心。
pub(crate) fn grid_height(units: &[Unit], avail: f32) -> Option<f32> {
    let line_h = body_line(units)?;
    let rows = (avail / line_h) as usize;
    if rows == 0 {
        return None;
    }
    Some(line_h * rows as f32)
}

/// 正文行高：出现次数最多的那一档行高。
///
/// 标题、代码块、行内图是少数派，不影响「版心 = N 行」这个判断；页级 DP 也
/// 用它当尺子——「页底还剩几行」这个数必须按正文的行来量。
pub(crate) fn body_line(units: &[Unit]) -> Option<f32> {
    const MAX_BUCKETS: usize = 16;
    let mut buckets: [Option<(f32, u32)>; MAX_BUCKETS] = [None; MAX_BUCKETS];
    let mut used = 0usize;
    for unit in units {
        if !unit.line || unit.h <= 0.0 {
            continue;
        }
        let mut found = false;
        for slot in buckets[..used].iter_mut().flatten() {
            if slot.0 == unit.h {
                slot.1 += 1;
                found = true;
                break;
            }
        }
        if !found && used < MAX_BUCKETS {
            buckets[used] = Some((unit.h, 1));
            used += 1;
        }
    }
    // 平局取先出现的那一档（行按 y 排序，所以「先出现」也是确定的）。
    let mut best: Option<(f32, u32)> = None;
    for slot in buckets[..used].iter().flatten() {
        if best.is_none_or(|b| slot.1 > b.1) {
            best = Some(*slot);
        }
    }
    best.map(|(line_h, _)| line_h)
}

pub(crate) struct OpenPage {
    pub(crate) index: u32,
    pub(crate) top: f32,
    pub(crate) extra: f32,
    pub(crate) op_start: u32,
    pub(crate) line_start: u32,
    pub(crate) anchor: Option<ebook_ir::Anchor>,
}

pub(crate) struct PageBuilder {
    pub(crate) pages: Vec<Page>,
    pub(crate) page_size: SizeF,
    inset: EdgesF,
    open: Option<OpenPage>,
}

impl PageBuilder {
    pub(crate) fn new(page_size: SizeF, inset: EdgesF) -> Self {
        Self {
            pages: Vec::new(),
            page_size,
            inset,
            open: None,
        }
    }

    pub(crate) fn sync(&mut self, page: u32, top: f32, extra: f32, ops_len: u32, lines_len: u32) {
        let same = self.open.as_ref().is_some_and(|o| o.index == page);
        if same {
            return;
        }
        self.close(ops_len, lines_len);
        self.open = Some(OpenPage {
            index: page,
            top,
            extra,
            op_start: ops_len,
            line_start: lines_len,
            anchor: None,
        });
    }

    pub(crate) fn set_anchor(&mut self, anchor: Option<&ebook_ir::Anchor>) {
        if let Some(o) = self.open.as_mut() {
            if o.anchor.is_none() {
                o.anchor = anchor.cloned();
            }
        }
    }

    pub(crate) fn close(&mut self, ops_len: u32, lines_len: u32) {
        let Some(o) = self.open.take() else { return };
        let op_len = ops_len.saturating_sub(o.op_start);
        let line_len = lines_len.saturating_sub(o.line_start);
        if op_len == 0 && line_len == 0 {
            return;
        }
        let mut size = self.page_size;
        // 突出版心的页：页盒本身长高一点，这样“每一行都在页盒内”这条不变量
        // 仍然成立，宿主不用为个别页面写特例。
        if o.extra > 0.0 {
            size.h += o.extra;
        }
        self.pages.push(Page {
            index: o.index,
            origin: PointF::new(0.0, o.top),
            size,
            inset: self.inset,
            ops: ebook_ir::Span::new(o.op_start, op_len),
            lines: ebook_ir::Span::new(o.line_start, line_len),
            anchor: o.anchor,
        });
    }
}

pub(crate) fn op_bounds(op: &Op) -> (f32, f32) {
    match op {
        Op::GlyphRun(run) => (run.origin.y - run.size, run.size * 1.4),
        Op::Rect { rect, .. } => (rect.y, rect.h),
        Op::Border(border) => (border.rect.y, border.rect.h),
        Op::Image { rect, .. } => (rect.y, rect.h),
        Op::Unresolved(u) => (u.rect.y, u.rect.h),
        Op::Clip { rect, .. } => (rect.y, rect.h),
        // 链接热区与行盒同形；它总是跟着 `Item::Line` 一起进 ops，不会单独成为分页单元。
        Op::Link { rect, .. } => (rect.y, rect.h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typography::PageCost;

    /// 一列等宽的行，`heights` 决定每行多高。
    fn rows(heights: &[f32]) -> Vec<Unit> {
        let mut y = 0.0;
        heights
            .iter()
            .enumerate()
            .map(|(index, h)| {
                let unit = Unit {
                    y,
                    h: *h,
                    line: true,
                    block: 1,
                    order: index as u16,
                    len: heights.len() as u16,
                    orphans: 1,
                    widows: 1,
                    force: false,
                    avoid_before: false,
                    avoid_after: false,
                };
                y += h;
                unit
            })
            .collect()
    }

    fn optimal() -> PageRules {
        PageRules {
            plan: PlanKind::Optimal,
            cost: PageCost::ZH,
            fixes: &[],
            push_lines: 1,
            align_lines: false,
            orphan_char: false,
            flush_bottom: false,
        }
    }

    fn cuts(plan: &Plan) -> Vec<usize> {
        plan.starts
            .iter()
            .map(|start| start.unit as usize)
            .collect()
    }

    /// 计划的代价，口径和 `plan_optimal` 完全一致（末页免费）。
    fn cost_of(units: &[Unit], plan: &Plan, fit_h: f32, line_h: f32, weight: PageCost) -> f32 {
        let n = units.len();
        let mut total = 0.0;
        for (index, start) in plan.starts.iter().enumerate() {
            let s = start.unit as usize;
            let e = plan
                .starts
                .get(index + 1)
                .map(|next| next.unit as usize)
                .unwrap_or(n);
            if e == n {
                break;
            }
            let used = units[e - 1].y + units[e - 1].h - units[s].y;
            total += page_cost(units, e, used, fit_h, line_h, weight);
        }
        total
    }

    /// 穷举所有切法，返回最小代价。只对小样本用。
    fn brute(units: &[Unit], fit_h: f32, line_h: f32, push: f32, weight: PageCost) -> f32 {
        fn walk(
            units: &[Unit],
            s: usize,
            fit_h: f32,
            line_h: f32,
            push: f32,
            weight: PageCost,
            acc: f32,
            best: &mut f32,
        ) {
            if s == units.len() {
                if acc < *best {
                    *best = acc;
                }
                return;
            }
            let n = units.len();
            let mut e = s;
            while e < n {
                if e > s && units[e].force {
                    break;
                }
                e += 1;
                let used = units[e - 1].y + units[e - 1].h - units[s].y;
                if used > fit_h + line_h * push + 0.001 && e > s + 1 {
                    break;
                }
                let cost = if e == n {
                    acc
                } else {
                    acc + page_cost(units, e, used, fit_h, line_h, weight)
                };
                if cost >= *best {
                    continue;
                }
                walk(units, e, fit_h, line_h, push, weight, cost, best);
            }
        }
        let mut best = f32::INFINITY;
        walk(units, 0, fit_h, line_h, push, weight, 0.0, &mut best);
        best
    }

    #[test]
    fn dp_matches_exhaustive_search() {
        // 高度参差、行数刚好卡在页边，逼出所有取舍。
        let heights = [
            24.0, 24.0, 24.0, 24.0, 24.0, 24.0, 24.0, 24.0, 24.0, 24.0, 24.0, 24.0,
        ];
        let units = rows(&heights);
        let rules = optimal();
        for fit in [72.0, 96.0, 120.0] {
            let plan = plan(&units, fit, fit, 24.0, 100, &rules);
            let mine = cost_of(&units, &plan, fit, 24.0, rules.cost);
            let best = brute(&units, fit, 24.0, 1.0, rules.cost);
            assert!(
                (mine - best).abs() < 0.001,
                "fit={fit}: DP {mine} vs 穷举 {best}"
            );
        }
    }

    #[test]
    fn dp_never_crosses_a_forced_break() {
        let mut units = rows(&[24.0; 20]);
        units[7].force = true;
        let rules = optimal();
        let plan = plan(&units, 96.0, 96.0, 24.0, 100, &rules);
        let cuts = cuts(&plan);
        assert!(cuts.contains(&7), "{cuts:?}");
    }

    #[test]
    fn dp_limits_push_out_to_the_configured_lines() {
        let mut units = rows(&[24.0; 24]);
        // 最后一行比别的都高：任何一页都不可能靠「多放一行」把它塞进去。
        units[23].h = 100.0;
        let rules = optimal();
        let limit = 96.0 + 24.0 * rules.push_lines as f32;
        let plan = plan(&units, 96.0, 96.0, 24.0, 100, &rules);
        for (index, start) in plan.starts.iter().enumerate() {
            let e = plan
                .starts
                .get(index + 1)
                .map(|next| next.unit as usize)
                .unwrap_or(units.len());
            let used = units[e - 1].y + units[e - 1].h - units[start.unit as usize].y;
            // 一页至少要放一个单元，所以超高图片那页只能放它自己。
            assert!(
                used <= limit + 0.001 || e == start.unit as usize + 1,
                "cuts={:?} used={used} es={:?}",
                cuts(&plan),
                (start.unit as usize, e)
            );
        }
    }

    /// 两道缝隙里摊平：剩 3px、缝隙 20px → 比例 0.15，每道缝各多 1.5px。
    #[test]
    fn flush_scale_spreads_the_slack() {
        let mut units = rows(&[24.0; 20]);
        for unit in &mut units[4..] {
            unit.y += 10.0;
        }
        for unit in &mut units[7..] {
            unit.y += 10.0;
        }
        let starts = alloc::vec![
            PageStart {
                unit: 0,
                y: 0.0,
                extra: 0.0
            },
            PageStart {
                unit: 10,
                y: units[10].y,
                extra: 0.0
            },
        ];
        let scale = flush_scale(&units, &starts, 263.0, 24.0);
        assert!((scale[0] - 0.15).abs() < 0.0001, "{scale:?}");
    }

    /// 每道缝最多撑 0.1 行：剩 140px 也只摊 2 × 2.4。
    #[test]
    fn flush_scale_caps_each_gap() {
        let mut units = rows(&[24.0; 20]);
        for unit in &mut units[4..] {
            unit.y += 10.0;
        }
        for unit in &mut units[7..] {
            unit.y += 10.0;
        }
        let starts = alloc::vec![
            PageStart {
                unit: 0,
                y: 0.0,
                extra: 0.0
            },
            PageStart {
                unit: 10,
                y: units[10].y,
                extra: 0.0
            },
        ];
        let scale = flush_scale(&units, &starts, 400.0, 24.0);
        assert!((scale[0] - 0.24).abs() < 0.0001, "{scale:?}");
    }

    /// 只有一道缝时不摊：那一整块零头会变成一条沟。
    #[test]
    fn flush_scale_skips_a_single_gap() {
        let mut units = rows(&[24.0; 20]);
        for unit in &mut units[4..] {
            unit.y += 10.0;
        }
        let starts = alloc::vec![
            PageStart {
                unit: 0,
                y: 0.0,
                extra: 0.0
            },
            PageStart {
                unit: 10,
                y: units[10].y,
                extra: 0.0
            },
        ];
        let scale = flush_scale(&units, &starts, 400.0, 24.0);
        assert_eq!(scale[0], 0.0);
    }

    /// 末页短是书排完了，不该被撑满。
    #[test]
    fn flush_scale_leaves_the_last_page_alone() {
        let mut units = rows(&[24.0; 20]);
        for unit in &mut units[4..] {
            unit.y += 10.0;
        }
        let starts = alloc::vec![PageStart {
            unit: 0,
            y: 0.0,
            extra: 0.0
        }];
        let scale = flush_scale(&units, &starts, 744.0, 24.0);
        assert_eq!(scale[0], 0.0);
    }

    #[test]
    fn dp_truncates_at_max_pages() {
        let units = rows(&[24.0; 40]);
        let rules = optimal();
        let plan = plan(&units, 96.0, 96.0, 24.0, 2, &rules);
        assert!(plan.truncated);
        assert_eq!(plan.starts.len(), 2);
        assert_eq!(plan.cut, Some(8));
    }
}
