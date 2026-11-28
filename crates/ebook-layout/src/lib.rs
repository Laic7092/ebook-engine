#![cfg_attr(not(test), no_std)]
// flow.rs 由另一条并行分支持有（硬化任务不改它），下面三条是那里的纯风格
// warning：拆参数结构体、去掉手写 push 都需要改调用点。等该分支合入后
// 统一修复，再删掉这段 allow；新代码不要依赖它。
#![allow(
    clippy::too_many_arguments,
    clippy::vec_init_then_push,
    clippy::neg_cmp_op_on_partial_ord
)]

extern crate alloc;
#[cfg(feature = "parallel")]
extern crate std;

mod cache;
mod flow;
pub mod hit;
mod hyphen;
mod linebreak;
mod linebreak_table;
mod measure;
mod paginate;
mod punct;
// 测试里总是编进来（`cargo test -p ebook-layout` 不开 feature 也能测到）；
// wasm 宿主不需要，所以发布构建仍然由 feature 挡住。
#[cfg(any(test, feature = "quality"))]
pub mod quality;
mod resolve;
pub mod select;
mod shape;
mod typography;

use alloc::string::String;
use alloc::vec::Vec;

use ebook_ir::{
    BorderBox, DisplayList, EdgesF, FontId, FontSpec, GlyphRun, ImageId, Issue, IssueCode,
    LayoutResult, Limits, LineBox, NodeId, NodeKind, Op, RectF, RunMap, SidesF, SizeF, StyledDoc,
    Unresolved, UnresolvedReason, WritingMode, IR_VERSION,
};

pub use cache::FlowCache;
pub use hit::Reader;
#[cfg(feature = "hyphen-en")]
pub use hyphen::English;
pub use hyphen::Hyphenator;
pub use measure::{ShapeGlyph, StubMeasurer, TextMeasurer};
pub use select::{Selection, SelectionRect};
pub use shape::shape_per_char;

pub trait ResourceView {
    fn image_size(&self, id: ImageId) -> Option<SizeF>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoImageSizes;

impl ResourceView for NoImageSizes {
    fn image_size(&self, _id: ImageId) -> Option<SizeF> {
        None
    }
}

#[derive(Clone, Copy)]
pub struct LayoutEnv<'a> {
    pub measurer: &'a dyn TextMeasurer,
    pub resources: &'a dyn ResourceView,
}

impl LayoutEnv<'_> {
    pub fn select_font(&self, spec: &FontSpec) -> FontId {
        self.measurer.select(spec)
    }

    pub fn font_for(&self, spec: &FontSpec, ch: char) -> FontId {
        self.measurer.select_char(spec, ch)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PageConstraint {
    pub page: SizeF,
    pub margin: EdgesF,
    pub writing_mode: WritingMode,
}

impl PageConstraint {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            page: SizeF::new(width, height),
            margin: EdgesF::ZERO,
            writing_mode: WritingMode::HorizontalTb,
        }
    }

    pub fn with_margin(mut self, margin: EdgesF) -> Self {
        self.margin = margin;
        self
    }

    pub fn content_size(&self) -> SizeF {
        SizeF::new(
            (self.page.w - self.margin.horizontal()).max(1.0),
            (self.page.h - self.margin.vertical()).max(1.0),
        )
    }
}

impl Default for PageConstraint {
    fn default() -> Self {
        Self::new(600.0, 800.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// 分页：`pages` 覆盖整章，宿主一页一页地翻。
    #[default]
    Paged,
    /// 滚动：整章排成**一页**，`Page.origin` 为 0，`Page.size.h` 是内容高度。
    /// 宿主按任意 `y` 取窗口（`ebook_render::paint_window`）画成瓦片。
    Scroll,
}

/// 一次排版的选项。零值表示“全部走默认”：`Limits` 里为 0 的字段由
/// [`Limits::resolved`] 换成默认上限。
/// 块级窗口：只排 `from..=to` 的文档节点子树，窗口外的块直接跳过。
///
/// `from` / `to` 是文档节点 id（同一章、同一份解析内稳定）。调用方通常先用
/// `StyledDoc::block_of` 把 `Anchor` 折到最近的块级节点，再传进来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockWindow {
    pub from: NodeId,
    pub to: NodeId,
}

/// 版式档案：语言知识的那张表（`typography.rs`）。
///
/// 默认（`None`）由 `StyledDoc.meta.lang` 选：简中走 `ZhHans`，其余走
/// `Generic`。宿主把它写成设置，就是替读者决定「按哪门语言的规矩排」——
/// 不动 `meta.lang` 本身，所以 `locl` 之类的字形选择仍随书。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// 零语言假设：断行贪心、分页贪心、不对齐。
    Generic,
    /// 简体中文（CLREQ）：行/页动态规划、两端对齐、孤字与孤行修复、页底齐平。
    ZhHans,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutOptions {
    pub limits: Limits,
    pub mode: Mode,
    /// `Some` 时只排这个块窗口；`FlowCache` 不会被使用（窗口是独立结果）。
    pub window: Option<BlockWindow>,
    /// `Some` 时用这张表，而不是按 `meta.lang` 选。
    pub profile: Option<Profile>,
}

impl LayoutOptions {
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            mode: Mode::Paged,
            window: None,
            profile: None,
        }
    }

    /// 滚动模式：整章一页，页高是实际内容高度。上下留白应由调用方传 0，
    /// 左右留白仍留在页里；宿主在滚动容器上自己加顶部/底部留白，瓦片之间
    /// 才不会出现空白带。
    pub fn scroll() -> Self {
        Self {
            limits: Limits::default(),
            mode: Mode::Scroll,
            window: None,
            profile: None,
        }
    }

    /// 只排一个块窗口；通常和 `Mode::Scroll` 一起用。
    pub fn window(mut self, from: NodeId, to: NodeId) -> Self {
        self.window = Some(BlockWindow { from, to });
        self
    }
}

#[derive(Default)]
pub struct Layout;

impl Layout {
    pub fn new() -> Self {
        Self
    }

    /// 默认预算：`Limits::default()`，与旧调用点行为一致。
    pub fn layout(
        &mut self,
        doc: &StyledDoc,
        env: &LayoutEnv<'_>,
        c: &PageConstraint,
    ) -> LayoutResult {
        self.layout_with(doc, env, c, &LayoutOptions::default())
    }

    /// 带预算的排版。超限时返回合法但被截断的 IR：`pages` 覆盖
    /// `display.ops` / `runs.lines` 仍成立，`truncated` 为真。
    pub fn layout_with(
        &mut self,
        doc: &StyledDoc,
        env: &LayoutEnv<'_>,
        c: &PageConstraint,
        opts: &LayoutOptions,
    ) -> LayoutResult {
        self.run(doc, env, c, opts, None, None)
    }

    /// 把“段落断行之后的工作单元”切成任务并行计算，再按输入顺序拼回。
    ///
    /// 输出必须是串行结果的逐字节副本：任务只写自己的下标，不共享可变状态。
    /// `workers` 只是本次调用的上限；线程在一次调用内创建、返回前 join。
    /// `parallel` feature 关闭时（wasm 就是这样）直接走串行路径。
    pub fn layout_parallel(
        &mut self,
        doc: &StyledDoc,
        env: &LayoutEnv<'_>,
        c: &PageConstraint,
        workers: usize,
        opts: &LayoutOptions,
    ) -> LayoutResult {
        let workers = workers.max(1);
        self.run(doc, env, c, opts, Some(workers), None)
    }

    /// 与前两个入口相同，但盒树 + 断行 + 整形可以跨调用复用。
    ///
    /// 分页依赖页高，断行只依赖可用宽度：所以**换个页高、换个模式**（分页 ↔
    /// 滚动）而宽度没变时，`flow` 直接命中，只剩分页与组装要跑。落地场景是
    /// 手机地址栏收起、旋屏、以及宿主切换分页/滚动。
    ///
    /// `rev` 是调用方的版本号，必须覆盖所有参与排版的**文档与字体字节**：
    /// 同一个 `rev` 下 `doc` 与 `env` 必须逐字节等价。改字、改样式、换字体、
    /// 换书都要换一个值（或者直接 [`FlowCache::clear`]）。给错了不会 panic，
    /// 只会拿到上一份 `flow`——这也是它只能由调用方显式传进来的原因。
    pub fn layout_cached(
        &mut self,
        doc: &StyledDoc,
        env: &LayoutEnv<'_>,
        c: &PageConstraint,
        opts: &LayoutOptions,
        cache: &mut FlowCache,
        rev: u64,
    ) -> LayoutResult {
        self.run(doc, env, c, opts, None, Some((cache, rev)))
    }

    fn run(
        &mut self,
        doc: &StyledDoc,
        env: &LayoutEnv<'_>,
        c: &PageConstraint,
        opts: &LayoutOptions,
        workers: Option<usize>,
        cache: Option<(&mut FlowCache, u64)>,
    ) -> LayoutResult {
        let limits = opts.limits.resolved();

        // 先量后遍历：arena 的节点数、文本字节数是 O(1) 的，深度按 parent 链
        // 记忆化一次算完。超限时连 flow 都不进。
        if doc.nodes.len() > limits.max_nodes as usize {
            return limited_result(doc, "max_nodes");
        }
        if doc_depth(doc) > limits.max_depth as u32 {
            return limited_result(doc, "max_depth");
        }
        if doc.text.len() > limits.max_text_bytes as usize {
            return limited_result(doc, "max_text_bytes");
        }

        let content = c.content_size();
        // 版式档案：宿主指了就用它，否则由 `meta.lang` 选。机制不认识具体
        // 语言，只查这张表。
        let typo = match opts.profile {
            Some(profile) => typography::of_profile(profile),
            None => typography::of_lang(doc.meta.lang.as_deref()),
        };
        // 键只含「断行真正依赖的东西」：调用方版本号、可用宽度、行数预算。
        // 页高与 `Mode` 刻意不进键——它们只影响分页。
        // 窗口结果是独立坐标（已归一化到 0 附近），不能写进整章 FlowCache。
        let mut built = if let Some(window) = opts.window {
            flow::flow(doc, env, c, limits.max_lines, typo, Some(window))
        } else {
            match cache {
                Some((cache, rev)) => {
                    let key = cache::flow_key(rev, content.w, limits.max_lines);
                    match cache.get(key) {
                        Some(flow) => flow,
                        None => {
                            let flow = flow::flow(doc, env, c, limits.max_lines, typo, None);
                            cache.put(key, &flow);
                            flow
                        }
                    }
                }
                None => flow::flow(doc, env, c, limits.max_lines, typo, None),
            }
        };
        let flow_truncated = built.truncated;

        // 分页本身按行串行（页起点依赖上一页终点），但把每个 item 换成分页
        // 单元的映射是纯函数，可以按任务切。任务只写自己的下标，拼回时顺序固定。
        let units = match workers {
            Some(workers) => units_of_parallel(&built.items, workers),
            None => units_of(&built.items),
        };
        // 滚动模式的页高：整章内容实际占的高度（最后一行/图片/块边框的底边），
        // 不是视口高度。块级 padding/border 不在行单元里，所以这里把装饰也
        // 算进去，否则滚动到底部时容器的下边框会被裁掉。
        let mut flow_height = units.iter().map(|unit| unit.y + unit.h).fold(0.0, f32::max);
        for op in &built.decorations {
            let (y, h) = paginate::op_bounds(op);
            flow_height = flow_height.max(y + h);
        }
        // 版心高度：分页模式下若档案要求纵横对齐，就取正文行高的整数倍，
        // 余数沉到页脚。`Page.size.h` 仍然是宿主给的版心高度，只是最后那点
        // 不足一行的空间不再放行——页盒、位图、坐标都不变。
        let line_h = paginate::body_line(&units).unwrap_or(content.h);
        let fit_h = match opts.mode {
            Mode::Paged if typo.page.align_lines => {
                paginate::grid_height(&units, content.h).unwrap_or(content.h)
            }
            _ => content.h,
        };
        let plan = match opts.mode {
            Mode::Paged => paginate::plan(
                &units,
                fit_h,
                content.h,
                line_h,
                limits.max_pages as usize,
                &typo.page,
            ),
            Mode::Scroll => paginate::plan_scroll(),
        };

        let page_size = match opts.mode {
            Mode::Paged => content,
            Mode::Scroll => SizeF::new(content.w, flow_height),
        };
        let mut builder = paginate::PageBuilder::new(page_size, c.margin);
        let mut ops: Vec<Op> = Vec::new();
        let mut lines: Vec<LineBox> = Vec::new();
        let mut issues: Vec<Issue> = Vec::new();
        let mut truncated = plan.truncated || flow_truncated;
        if plan.truncated {
            issues.push(Issue {
                node: doc.root,
                code: IssueCode::LimitExceeded,
                detail: Some(String::from("max_pages")),
            });
        }
        if flow_truncated {
            issues.push(Issue {
                node: doc.root,
                code: IssueCode::LimitExceeded,
                detail: Some(String::from("max_lines")),
            });
        }

        let max_lines = limits.max_lines as usize;
        let max_image_pixels = limits.max_image_pixels as u64;
        let mut image_pixels = 0u64;
        let mut image_nodes: Option<Vec<NodeId>> = None;

        // 背景/边框按页切段。`plan.starts` 已经定好页边界，装饰 op 又不参与
        // 分页，所以这里可以纯函数地算出每一页该画哪一段；等该页的第一条
        // 内容打开页时，再把段塞到内容之前。
        let mut decorations: Vec<Vec<Op>> = (0..plan.starts.len()).map(|_| Vec::new()).collect();
        for op in &built.decorations {
            split_decoration(op, &plan.starts, content.h, opts.mode, &mut decorations);
        }

        // 页底齐平：每页的拉伸比例，组装时按「空隙 × 比例」把后面的内容往下推。
        // 档案没开时是空表，走原来的路径，一个字节都不变。
        let mut flush = if opts.mode == Mode::Paged && typo.page.flush_bottom {
            paginate::flush_scale(&units, &plan.starts, fit_h, line_h)
        } else {
            Vec::new()
        };
        // 有背景/边框的页不拉伸：装饰 op 是按块算的，跟着字一起挪就得知道
        // 它挂在哪一段上；不挪则边框和里面的字错位。宁可这一页不齐。
        for (page, list) in decorations.iter().enumerate() {
            if !list.is_empty() && page < flush.len() {
                flush[page] = 0.0;
            }
        }
        let mut shift = 0.0f32;
        let mut shift_page = u32::MAX;

        let mut open_page: Option<u32> = None;
        for (index, item) in built.items.drain(..).enumerate() {
            if plan.cut.is_some_and(|cut| index >= cut) {
                truncated = true;
                break;
            }
            // 断行在 flow 里已经发生；这里在组装侧“停止再产”，已产出的行保留。
            // `flow_truncated` 已经在上面记过诊断，这里不重复报同一个超限。
            if lines.len() >= max_lines {
                truncated = true;
                if !flow_truncated {
                    issues.push(Issue {
                        node: doc.root,
                        code: IssueCode::LimitExceeded,
                        detail: Some(String::from("max_lines")),
                    });
                }
                break;
            }
            let page = paginate::page_of(&plan.starts, index);
            if page != shift_page {
                shift_page = page;
                shift = 0.0;
            } else if let Some(scale) = flush.get(page as usize).copied() {
                if scale > 0.0 {
                    let gap = units[index].y - (units[index - 1].y + units[index - 1].h);
                    if gap > 0.5 {
                        shift += gap * scale;
                    }
                }
            }
            if open_page != Some(page) {
                let top = plan.starts[page as usize].y;
                let extra = plan.starts[page as usize].extra;
                builder.sync(page, top, extra, ops.len() as u32, lines.len() as u32);
                if let Some(list) = decorations.get_mut(page as usize) {
                    ops.append(list);
                }
                open_page = Some(page);
            }
            match item {
                flow::Item::Line(mut line) => {
                    if shift != 0.0 {
                        line.y += shift;
                        line.baseline += shift;
                        for run in &mut line.runs {
                            run.origin.y += shift;
                        }
                        for op in &mut line.unresolved {
                            shift_op(op, shift);
                        }
                    }
                    builder.set_anchor(line.anchor.as_ref());
                    // `item` 已经是 owned（`drain` 出来的），run 文本直接 move，
                    // 不再 `clone` 一份；大章节上少一半字符串分配。
                    for run in line.runs {
                        ops.push(Op::GlyphRun(GlyphRun {
                            font: run.font,
                            size: run.size,
                            color: run.color,
                            origin: run.origin,
                            glyphs: run.glyphs,
                            text: run.text,
                            dir: run.dir,
                            rotate: run.rotate,
                            weight: run.weight,
                            style: run.style,
                            underline: run.underline,
                        }));
                    }
                    for op in line.unresolved {
                        if let Op::Unresolved(unresolved) = &op {
                            issues.push(issue_of(unresolved));
                        }
                        ops.push(op);
                    }
                    if let Some(href) = line.link {
                        ops.push(Op::Link {
                            rect: RectF::new(line.x, line.y, line.w, line.h),
                            href,
                        });
                    }
                    lines.push(LineBox {
                        rect: RectF::new(line.x, line.y, line.w, line.h),
                        baseline: line.baseline,
                        dir: line.dir,
                        src: line.src,
                        anchor: line.anchor,
                        glyphs: line.glyphs,
                        node: line.node,
                    });
                }
                flow::Item::Op(Op::Image { id, rect, fit }) => {
                    let rect = RectF::new(rect.x, rect.y + shift, rect.w, rect.h);
                    // 图片像素预算：按资源给出的固有尺寸累加，超限的图降级成
                    // `SizeFromContent`，交给宿主决定怎么办，而不是让排版去猜。
                    let size = env
                        .resources
                        .image_size(id)
                        .unwrap_or(SizeF::new(rect.w, rect.h));
                    let pixels = (size.w.max(0.0) as u64).saturating_mul(size.h.max(0.0) as u64);
                    if image_pixels.saturating_add(pixels) > max_image_pixels {
                        let node = image_node(doc, &mut image_nodes, id);
                        issues.push(Issue {
                            node,
                            code: IssueCode::LimitExceeded,
                            detail: Some(String::from("max_image_pixels")),
                        });
                        ops.push(Op::Unresolved(Unresolved {
                            node,
                            rect,
                            reason: UnresolvedReason::SizeFromContent,
                        }));
                        truncated = true;
                    } else {
                        image_pixels += pixels;
                        ops.push(Op::Clip {
                            rect,
                            ops: alloc::vec![Op::Image { id, rect, fit }],
                        });
                    }
                }
                flow::Item::Op(mut op) => {
                    if shift != 0.0 {
                        shift_op(&mut op, shift);
                    }
                    if let Op::Unresolved(unresolved) = &op {
                        issues.push(issue_of(unresolved));
                    }
                    ops.push(op);
                }
            }
        }
        builder.close(ops.len() as u32, lines.len() as u32);

        // 滚动模式被 `max_lines` 截断时，页高只算真正产出的行，不把没排的
        // 内容也算进去：宿主拿到的是一个可滚动的真实高度。
        if opts.mode == Mode::Scroll && truncated {
            if let Some(page) = builder.pages.first_mut() {
                page.size.h = lines
                    .iter()
                    .map(|line| line.rect.bottom())
                    .fold(0.0, f32::max);
            }
        }

        // 截断后丢弃尾部不再被行引用的字形，保证 IR 里没有悬空的字形区间。
        if truncated {
            // 行区间之外还可能有字形（列表 marker 自成一个 run、不并进
            // `LineBox.glyphs`），截断的上界要把 run 的区间也算上，否则会留下
            // 指向已删字形的 run。
            let run_end = ops
                .iter()
                .filter_map(|op| match op {
                    Op::GlyphRun(run) => Some(run.glyphs.end()),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            let glyph_end = lines
                .iter()
                .map(|line| line.glyphs.end())
                .max()
                .unwrap_or(0)
                .max(run_end);
            built.glyphs.truncate(glyph_end as usize);
        }

        LayoutResult {
            version: IR_VERSION,
            spine: doc.meta.spine,
            pages: builder.pages,
            display: DisplayList {
                ops,
                glyphs: built.glyphs,
            },
            runs: RunMap { lines },
            blocks: built.blocks,
            dirty: built.dirty,
            issues,
            truncated,
        }
    }
}

/// 未解析占位 → 诊断条目。
fn issue_of(unresolved: &Unresolved) -> Issue {
    let code = match unresolved.reason {
        UnresolvedReason::ImagePending => IssueCode::MissingImage,
        UnresolvedReason::FontMissing => IssueCode::MissingGlyph,
        UnresolvedReason::SizeFromContent => IssueCode::SizeFromContent,
    };
    Issue {
        node: unresolved.node,
        code,
        detail: None,
    }
}

/// 限额触发的空结果。空结果是合法 IR：没有页也就没有页覆盖问题。
fn limited_result(doc: &StyledDoc, detail: &str) -> LayoutResult {
    LayoutResult {
        version: IR_VERSION,
        spine: doc.meta.spine,
        pages: Vec::new(),
        display: DisplayList::default(),
        runs: RunMap::default(),
        blocks: Vec::new(),
        dirty: Vec::new(),
        issues: alloc::vec![Issue {
            node: doc.root,
            code: IssueCode::LimitExceeded,
            detail: Some(String::from(detail)),
        }],
        truncated: true,
    }
}

/// 文档最大深度（根为 0）。按 parent 链记忆化，不递归，深文档不会爆栈。
fn doc_depth(doc: &StyledDoc) -> u32 {
    const UNKNOWN: u32 = u32::MAX;
    let n = doc.nodes.len();
    let mut memo = alloc::vec![UNKNOWN; n];
    let mut path: Vec<usize> = Vec::new();
    for start in 0..n {
        if memo[start] != UNKNOWN {
            continue;
        }
        path.clear();
        let mut cur = start;
        loop {
            if memo[cur] != UNKNOWN {
                break;
            }
            path.push(cur);
            match doc.nodes[cur].parent {
                Some(parent) if (parent as usize) < n && parent as usize != cur => {
                    cur = parent as usize
                }
                _ => {
                    memo[cur] = 0;
                    break;
                }
            }
            // parent 链成环（损坏的 arena）：把这条路当成超深。
            if path.len() > n {
                for &node in &path {
                    memo[node] = u32::MAX;
                }
                path.clear();
                break;
            }
        }
        while let Some(node) = path.pop() {
            if memo[node] != UNKNOWN {
                continue;
            }
            let parent = doc.nodes[node].parent.map(|p| p as usize).unwrap_or(node);
            let base = if parent < n && parent != node {
                memo[parent]
            } else {
                0
            };
            memo[node] = base.saturating_add(1);
        }
    }
    memo.into_iter()
        .filter(|&depth| depth != UNKNOWN)
        .max()
        .unwrap_or(0)
}

/// 按图片 id 找到声明它的节点，供限额降级时填 `Issue.node`。
fn image_node(doc: &StyledDoc, cache: &mut Option<Vec<NodeId>>, id: ImageId) -> NodeId {
    let map = cache.get_or_insert_with(|| {
        let mut owners = alloc::vec![doc.root; doc.images.len()];
        for (index, node) in doc.nodes.iter().enumerate() {
            if let NodeKind::Image(image) = node.kind {
                if let Some(owner) = owners.get_mut(image as usize) {
                    *owner = index as NodeId;
                }
            }
        }
        owners
    });
    map.get(id as usize).copied().unwrap_or(doc.root)
}

/// 把块级背景/边框切成每页一段。
///
/// 分页单元只看真实内容，所以一个跨页的边框盒不会自己撑开一页；它只是
/// 在每一页出现自己落在该页内的那一段。首/尾段保留上下边框，中间段只留
/// 左右边框，这样跨页时不会在页顶/页底多出横线。
fn split_decoration(
    op: &Op,
    starts: &[paginate::PageStart],
    page_h: f32,
    mode: Mode,
    out: &mut [Vec<Op>],
) {
    let rect = match op {
        Op::Rect { rect, .. } => *rect,
        Op::Border(border) => border.rect,
        _ => return,
    };
    if mode == Mode::Scroll || starts.len() <= 1 {
        if let Some(first) = out.first_mut() {
            first.push(op.clone());
        }
        return;
    }
    if rect.w <= 0.0 || rect.h < 0.0 {
        return;
    }

    let mut page = starts
        .iter()
        .rposition(|start| start.y <= rect.y + 0.001)
        .unwrap_or(0);
    while page < starts.len() {
        let top = starts[page].y;
        // 突出版心的页（`PageStart::extra`）比版心高一点，切段要跟着长高，
        // 否则页底那一条边框会被切掉。
        let page_bottom = top + page_h + starts[page].extra;
        let bottom = starts
            .get(page + 1)
            .map_or(page_bottom, |next| next.y.min(page_bottom).max(top));
        let seg_top = rect.y.max(top);
        let seg_bottom = rect.bottom().min(bottom);
        if seg_bottom > seg_top {
            let segment = RectF::new(rect.x, seg_top, rect.w, seg_bottom - seg_top);
            let piece = match op {
                Op::Rect { color, .. } => Op::Rect {
                    rect: segment,
                    color: *color,
                },
                Op::Border(border) => {
                    // 第一段才有上边框，最后一段才有下边框。
                    let first = segment.y <= border.rect.y + 0.001;
                    let last = segment.bottom() >= border.rect.bottom() - 0.001;
                    Op::Border(BorderBox {
                        rect: segment,
                        width: SidesF {
                            top: if first { border.width.top } else { 0.0 },
                            bottom: if last { border.width.bottom } else { 0.0 },
                            left: border.width.left,
                            right: border.width.right,
                        },
                        color: border.color,
                    })
                }
                _ => return,
            };
            out[page].push(piece);
        }
        if rect.bottom() <= bottom + 0.001 {
            break;
        }
        page += 1;
    }
}

fn unit_of(item: &flow::Item) -> paginate::Unit {
    match item {
        flow::Item::Line(line) => paginate::Unit {
            y: line.y,
            h: line.h,
            line: true,
            block: line.block,
            order: line.order,
            len: line.len,
            orphans: line.orphans,
            widows: line.widows,
            force: line.force,
            avoid_before: line.avoid_before,
            avoid_after: line.avoid_after,
        },
        flow::Item::Op(op) => {
            let (y, h) = paginate::op_bounds(op);
            paginate::Unit {
                y,
                h,
                line: false,
                block: u32::MAX,
                order: 0,
                len: 0,
                orphans: 1,
                widows: 1,
                force: false,
                avoid_before: false,
                avoid_after: false,
            }
        }
    }
}

fn units_of(items: &[flow::Item]) -> Vec<paginate::Unit> {
    let mut units = Vec::with_capacity(items.len());
    for item in items {
        units.push(unit_of(item));
    }
    units
}

/// 并行版本：输入是只读的 item 切片，任务只写自己下标的结果槽。
/// 用 `thread::scope` + 原子索引队列，调用线程也参与，返回前 join。
#[cfg(feature = "parallel")]
fn units_of_parallel(items: &[flow::Item], workers: usize) -> Vec<paginate::Unit> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let n = items.len();
    // 任务数就是上限；再套一个固定上限，避免调用方传 1e6 时真去起 1e6 个线程。
    let workers = workers.clamp(1, 64).min(n.max(1));
    if workers <= 1 || n == 0 {
        return units_of(items);
    }
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<paginate::Unit>>> = Mutex::new((0..n).map(|_| None).collect());
    let claim = |next: &AtomicUsize| loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        if index >= n {
            return;
        }
        let unit = unit_of(&items[index]);
        slots.lock().unwrap()[index] = Some(unit);
    };
    std::thread::scope(|scope| {
        for _ in 1..workers {
            scope.spawn(|| claim(&next));
        }
        claim(&next);
    });
    slots
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|unit| unit.expect("every task wrote its slot"))
        .collect()
}

/// `parallel` 关闭时（wasm）`layout_parallel` 退化成串行，签名保持不变。
#[cfg(not(feature = "parallel"))]
fn units_of_parallel(items: &[flow::Item], _workers: usize) -> Vec<paginate::Unit> {
    units_of(items)
}

pub fn layout(doc: &StyledDoc, env: &LayoutEnv<'_>, c: &PageConstraint) -> LayoutResult {
    Layout::new().layout(doc, env, c)
}

/// 把一个 op 整体下移 `dy`（页底齐平把行往下推时，图和它的热区要一起走）。
fn shift_op(op: &mut Op, dy: f32) {
    match op {
        Op::GlyphRun(run) => run.origin.y += dy,
        Op::Rect { rect, .. } => rect.y += dy,
        Op::Border(border) => border.rect.y += dy,
        Op::Image { rect, .. } => rect.y += dy,
        Op::Unresolved(unresolved) => unresolved.rect.y += dy,
        Op::Clip { rect, ops } => {
            rect.y += dy;
            for inner in ops.iter_mut() {
                shift_op(inner, dy);
            }
        }
        Op::Link { rect, .. } => rect.y += dy,
    }
}
