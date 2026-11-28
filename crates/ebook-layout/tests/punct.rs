//! 中文排版政策：成对标点挤压、行末悬挂、中西文自动间距。
//!
//! 三个开关默认关闭；这里逐个打开验证可观测结果，关闭时的输出由 golden 守住。

use ebook_ir::doc::FontSpec;
use ebook_ir::{
    Anchor, ComputedStyle, Dir, FontId, LayoutResult, LineHeight, Node, NodeId, NodeKind, Op,
    Rgba8, Span, StyledDoc, Tag, TextRules, TextStyle,
};
use ebook_layout::{layout, LayoutEnv, NoImageSizes, PageConstraint, TextMeasurer};

struct Fixed;

impl TextMeasurer for Fixed {
    fn advance(&self, ch: char, _font: FontId, _size: f32) -> f32 {
        if ch == ' ' {
            4.0
        } else if ch.is_ascii_alphanumeric() {
            5.0
        } else {
            10.0
        }
    }

    fn ascent(&self, _font: FontId, _size: f32) -> f32 {
        10.0
    }

    fn descent(&self, _font: FontId, _size: f32) -> f32 {
        2.0
    }
}

fn block_style() -> ComputedStyle {
    ComputedStyle {
        font: FontSpec {
            font: FontId(0),
            size: 10.0,
            weight: 400,
            ..FontSpec::default()
        },
        line_height: LineHeight::Px(20.0),
        ..ComputedStyle::default()
    }
}

fn inline_style(rules: TextRules) -> ComputedStyle {
    let mut style = ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 10.0,
            weight: 400,
            ..FontSpec::default()
        },
        Rgba8::BLACK,
        Dir::Ltr,
    );
    style.text = TextStyle {
        rules,
        ..TextStyle::default()
    };
    style
}

fn push(
    doc: &mut StyledDoc,
    kind: NodeKind,
    style: ComputedStyle,
    parent: Option<NodeId>,
) -> NodeId {
    doc.push_node(Node {
        kind,
        style,
        parent,
        first_child: 0,
        child_len: 0,
        src: Some(Anchor::default()),
        link: None,
    })
}

fn set_kids(doc: &mut StyledDoc, parent: NodeId, kids: &[NodeId]) {
    doc.set_kids(parent, kids);
    for (index, &kid) in kids.iter().enumerate() {
        doc.node_mut(kid).parent = Some(parent);
        doc.node_mut(kid).src = Some(Anchor {
            spine: 0,
            path: [index as u16].to_vec(),
            offset: 0,
        });
    }
}

fn doc(body: &str, rules: TextRules) -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 0;
    let root = push(&mut doc, NodeKind::Element(Tag::Body), block_style(), None);
    doc.root = root;
    let p = push(&mut doc, NodeKind::Element(Tag::P), block_style(), None);
    let start = doc.text.len() as u32;
    doc.text.push_str(body);
    let text = push(
        &mut doc,
        NodeKind::Text(Span::new(start, body.len() as u32)),
        inline_style(rules),
        None,
    );
    set_kids(&mut doc, p, &[text]);
    set_kids(&mut doc, root, &[p]);
    doc
}

fn run(doc: &StyledDoc, width: f32) -> LayoutResult {
    let env = LayoutEnv {
        measurer: &Fixed,
        resources: &NoImageSizes,
    };
    layout(doc, &env, &PageConstraint::new(width, 800.0))
}

fn glyphs_per_line(ir: &LayoutResult) -> Vec<usize> {
    ir.runs
        .lines
        .iter()
        .map(|line| ir.display.glyph_slice(line.glyphs).len())
        .collect()
}

fn glyph_dx(ir: &LayoutResult, ch: u32) -> f32 {
    let mut found = None;
    for op in &ir.display.ops {
        if let Op::GlyphRun(run) = op {
            for glyph in ir.display.glyph_slice(run.glyphs) {
                if glyph.ch == ch {
                    found = Some(glyph.dx);
                }
            }
        }
    }
    found.unwrap_or_else(|| panic!("no glyph at {ch}"))
}

#[test]
fn trimming_halves_the_quote_pairs() {
    // 他说：“走。”——`：`+`“` 压 `“` 的左半宽，`。`+`”` 压 `”` 的右半宽。
    let body = "他说：“走。”";
    let plain = run(&doc(body, TextRules::default()), 400.0);
    let rules = TextRules {
        spacing_trim: true,
        ..TextRules::default()
    };
    let trimmed = run(&doc(body, rules), 400.0);

    assert_eq!(plain.runs.lines.len(), 1);
    assert!((plain.runs.lines[0].rect.w - 70.0).abs() < 0.01);
    // 只压拥有空白半边、且在可压集合里的那一个：`：“` 压 `“` 的左半宽。
    // `。”` 里的 `。` 不是可压的闭引号（`~/layx/render/punct.go` 的
    // trimmableClosing 只含 U+2019/201D/3009），所以 `”` 保持全宽。
    assert!((trimmed.runs.lines[0].rect.w - 65.0).abs() < 0.01);
    // 字节偏移：他 0、说 3、：6、“9、走 12、。15、”18。
    assert!((glyph_dx(&trimmed, 9) - 5.0).abs() < 0.01, "opening quote");
    assert!(
        (glyph_dx(&trimmed, 18) - 10.0).abs() < 0.01,
        "closing quote"
    );
    // 不在表里的标点不压。
    assert!((glyph_dx(&trimmed, 6) - 10.0).abs() < 0.01);
    assert!((glyph_dx(&trimmed, 15) - 10.0).abs() < 0.01);
}

#[test]
fn adjacent_brackets_do_not_trim_without_a_pair() {
    // 「你好」没有相邻标点对：挤压开关打开也不改宽度（规则表只压引号类）。
    let body = "「你好」";
    let plain = run(&doc(body, TextRules::default()), 400.0);
    let rules = TextRules {
        spacing_trim: true,
        ..TextRules::default()
    };
    let trimmed = run(&doc(body, rules), 400.0);
    assert!((plain.runs.lines[0].rect.w - trimmed.runs.lines[0].rect.w).abs() < 0.01);
}

#[test]
fn hanging_keeps_the_closing_mark_on_the_line() {
    let body = "「你好」";
    // 40 宽只差一个标点：100 宽放得下，35 宽需要悬挂半宽才放得下。
    let plain = run(&doc(body, TextRules::default()), 35.0);
    assert_eq!(plain.runs.lines.len(), 2, "不开悬挂时标点被挤到下一行");

    let rules = TextRules {
        hanging_punctuation: true,
        ..TextRules::default()
    };
    let hanging = run(&doc(body, rules), 35.0);
    assert_eq!(hanging.runs.lines.len(), 1);
    // 行盒右边界跟着探出，字形坐标不变。
    assert!((hanging.runs.lines[0].rect.w - 40.0).abs() < 0.01);
    assert!((glyph_dx(&hanging, 9) - 10.0).abs() < 0.01);
}

#[test]
fn hanging_follows_the_language_profile() {
    // 同一个字符串、同一个宽度，只改 `meta.lang`：
    // 闭引号在通用档案里可以悬挂，在简中档案里不行（CLREQ §6.1.3 只允许点号类）。
    let rules = TextRules {
        hanging_punctuation: true,
        ..TextRules::default()
    };
    let body = "「你好」";
    let mut generic = doc(body, rules);
    assert_eq!(run(&generic, 35.0).runs.lines.len(), 1);
    generic.meta.lang = Some("zh".to_string());
    assert_eq!(run(&generic, 35.0).runs.lines.len(), 2);

    // 句号属于点号类，两个档案里都能悬挂。
    let body = "你好世，";
    let mut zh = doc(body, rules);
    zh.meta.lang = Some("zh".to_string());
    assert_eq!(run(&zh, 35.0).runs.lines.len(), 1);
}

/// CLREQ §7.1.2 的孤字：末行只剩一个字时，从上一行取一个字下来，上一行均排。
#[test]
fn single_char_last_line_is_pulled_back() {
    // 21 个汉字、每行 10 个：末行只剩最后一个字。
    let body = "一二三四五六七八九十一二三四五六七八九十甲";
    let generic = doc(body, TextRules::default());
    assert_eq!(glyphs_per_line(&run(&generic, 100.0)), vec![10, 10, 1]);

    let mut zh = doc(body, TextRules::default());
    zh.meta.lang = Some("zh".to_string());
    let ir = run(&zh, 100.0);
    // 末行多了一个字（一种切法是 10/9/2，另一种是 9/10/2——两行里少一个字的
    // 那行靠均排撑满，观感一样，所以这里只钉住「末行不再是孤字」和「撑满」）。
    let per_line = glyphs_per_line(&ir);
    assert_eq!(per_line.len(), 3, "{per_line:?}");
    assert_eq!(per_line[2], 2, "{per_line:?}");
    assert_eq!(per_line[0] + per_line[1], 19, "{per_line:?}");
    // 少一个字的那一行被均排撑满：行宽仍然是版心宽。
    assert!((ir.runs.lines[0].rect.w - 100.0).abs() < 0.01);
    assert!((ir.runs.lines[1].rect.w - 100.0).abs() < 0.01);
}

/// 中文档案把孤字拉回来，挪下来的整块不能以标点开头（避头尾）。
///
/// 上一行以「」」结尾时，退一个位置（断在「九」前面）是合法的，于是挪下来的是
/// 「九」」」两个字——末行以「九」开头，禁则没破，还多得了一个字。只挪一个字的
/// 老做法在这里会被禁则挡住，然后什么都不做。
#[test]
fn orphan_pull_moves_a_whole_punctuation_block() {
    let zh = |body: &str| {
        let mut d = doc(body, TextRules::default());
        d.meta.lang = Some("zh".to_string());
        glyphs_per_line(&run(&d, 100.0))
    };
    // 控制组：整行 10 个字，末行只剩「甲」→ 拉一个「十」下来。
    assert_eq!(zh("一二三四五六七八九十甲"), vec![9, 2]);
    // 上一行以「」」结尾：只挪「」」会违反避头尾，所以连它前面的「九」一起挪。
    assert_eq!(zh("一二三四五六七八九」甲"), vec![8, 3]);
}

/// CLREQ §6.2.2.3 的压缩侧：上一行退不动时，把末行整个收进上一行（压行）。
///
/// 一行 40 个拉丁字母（200px）加一个句号：句号放不下，而这串字母里没有任何
/// 合法断点，退无可退。档案给字缝留了 0.05em 的收紧余量，39 道缝够收紧
/// 19.5px，收下 10px 的句号绰绰有余——于是整段变成一行，行宽仍然正好 200。
#[test]
fn orphan_pull_compresses_the_line_when_it_cannot_move_back() {
    let body = format!("{}。", "abcdefghij".repeat(4));
    let mut zh = doc(&body, TextRules::default());
    zh.meta.lang = Some("zh".to_string());
    let ir = run(&zh, 200.0);
    assert_eq!(glyphs_per_line(&ir), vec![41]);
    assert!(
        (ir.runs.lines[0].rect.w - 200.0).abs() < 0.01,
        "{}",
        ir.runs.lines[0].rect.w
    );

    // generic 不给字缝收紧的余量（`gap_shrink = 0`），只能留着孤字。
    let generic = run(&doc(&body, TextRules::default()), 200.0);
    assert_eq!(glyphs_per_line(&generic), vec![40, 1]);
}

/// 行级 DP（Knuth–Plass）在西文上真的换了一种断法。
///
/// 中文等宽字看不出差别（每行都能靠均排填满，代价全是零），所以这条只测西文：
/// 词距有真实代价，整段一起挑断点才会与贪心不同。`uneven` 这段的词长参差，
/// 贪心会把某几行填满、把代价留给下一行，DP 则会往回挪一个词把两行摊匀。
#[test]
fn zh_line_dp_balances_latin_text() {
    let body =
        "the righteous melody of a quiet afternoon is not the same as the noise of a busy market "
            .repeat(4);
    let generic = glyphs_per_line(&run(&doc(&body, TextRules::default()), 200.0));
    let mut zh = doc(&body, TextRules::default());
    zh.meta.lang = Some("zh".to_string());
    let optimal = glyphs_per_line(&run(&zh, 200.0));
    assert_ne!(generic, optimal, "行级 DP 没起作用");
    // 行数不变：DP 换的是「在哪断」，不是「断几次」。
    assert_eq!(generic.len(), optimal.len());

    // 同样的输入必须得到同样的断法：DP 里没有哈希顺序，也没有跨行的浮点累积。
    let mut again = doc(&body, TextRules::default());
    again.meta.lang = Some("zh".to_string());
    assert_eq!(glyphs_per_line(&run(&again, 200.0)), optimal);
}

/// CLREQ §6.3.2.3：全角标点落在行末时压到半宽，于是整行刚好齐平，
/// 而不是把标点和它前面的字一起推到下一行。
#[test]
fn line_end_punctuation_is_squeezed_to_half_width() {
    // 9 个字（90）之后再来一个「。」：整字放不下（100 > 95），半宽刚好。
    let body = "一二三四五六七八九。";
    let generic = run(&doc(body, TextRules::default()), 95.0);
    // 避头尾不让断点落在「九」和「。」之间，于是「九。」一起被推下去。
    assert_eq!(glyphs_per_line(&generic), vec![8, 2]);

    let mut zh = doc(body, TextRules::default());
    zh.meta.lang = Some("zh".to_string());
    let ir = run(&zh, 95.0);
    // 标点留在行末，整行刚好齐平。
    assert_eq!(glyphs_per_line(&ir), vec![10]);
    assert!((ir.runs.lines[0].rect.w - 95.0).abs() < 0.01);
    // 压的只是前进宽度：字形本身没有被缩放。
    let last = ir
        .display
        .glyph_slice(ir.runs.lines[0].glyphs)
        .last()
        .unwrap();
    assert!((last.dx - 5.0).abs() < 0.01);
}

/// 闭括号不参与行末挤压：压半宽会露出半个空格。
#[test]
fn line_end_squeeze_skips_closing_brackets() {
    // 后面再跟一个字，避免被孤字处理拉回来，只看挤压本身。
    let body = "一二三四五六七八九」甲";
    let mut zh = doc(body, TextRules::default());
    zh.meta.lang = Some("zh".to_string());
    assert_eq!(glyphs_per_line(&run(&zh, 95.0)), vec![8, 3]);
}

/// CLREQ §6.2.2 / §6.3.1.2 均排：中文正文默认两端对齐，余量在各字之间均摊。
#[test]
fn chinese_body_is_justified_and_evenly_spaced() {
    let body = "一二三四五六七八九十一二三四五六七八九十一二三";
    // 每行 9 个字（90），版心 95：余量 5 摊到 8 个字之间。
    let mut zh = doc(body, TextRules::default());
    zh.meta.lang = Some("zh".to_string());
    let ir = run(&zh, 95.0);
    let first = &ir.runs.lines[0];
    assert!((first.rect.w - 95.0).abs() < 0.01);
    let dx: Vec<f32> = ir
        .display
        .glyph_slice(first.glyphs)
        .iter()
        .map(|g| g.dx)
        .collect();
    // 除最后一个字形外，每个字的前进宽度都一样：这就是「均排」。
    for pair in dx.windows(2) {
        assert!((pair[0] - 10.625).abs() < 0.01, "{dx:?}");
    }
    // 末行不拉伸。
    let last = ir.runs.lines.last().unwrap();
    assert!(last.rect.w < 95.0);

    // 没有 lang 的文档保持 CSS 初始值：左对齐，不拉伸。
    let plain = run(&doc(body, TextRules::default()), 95.0);
    assert!(plain.runs.lines[0].rect.w < 90.01);
}

#[test]
fn autospacing_inserts_a_quarter_em_between_cjk_and_latin() {
    let body = "中文English混排";
    let plain = run(&doc(body, TextRules::default()), 400.0);
    // 中(10) 文(10) E n g l i s h(7×5) 混(10) 排(10)
    assert!((plain.runs.lines[0].rect.w - 75.0).abs() < 0.01);

    let rules = TextRules {
        autospacing: true,
        ..TextRules::default()
    };
    let spaced = run(&doc(body, rules), 400.0);
    // 两处 1/4 em = 2 × 2.5。
    assert!((spaced.runs.lines[0].rect.w - 80.0).abs() < 0.01);
}

#[test]
fn autospacing_is_not_left_at_the_line_end() {
    let rules = TextRules {
        autospacing: true,
        ..TextRules::default()
    };
    let spaced = run(&doc("中a", rules), 10.0);
    assert_eq!(spaced.runs.lines.len(), 2);
    // 行末的空隙收回：中文行仍是 10，不是 12.5。
    assert!((spaced.runs.lines[0].rect.w - 10.0).abs() < 0.01);
    assert!((spaced.runs.lines[1].rect.w - 5.0).abs() < 0.01);
}

#[test]
fn rules_are_opt_in() {
    assert_eq!(
        TextRules::default(),
        TextRules {
            spacing_trim: false,
            hanging_punctuation: false,
            autospacing: false,
        }
    );
}
