use ebook_cache::{fingerprint, font_fingerprint, CacheKey};
use ebook_ir::{
    Anchor, ComputedStyle, Dir, Display, EdgesF, FontDesc, FontId, FontSet, FontSpec, FontStyle,
    ImageMeta, LayoutResult, Len, LineHeight, ListStyle, Node, NodeKind, Rgba8, SizeF, Span,
    StyledDoc, Tag, TextAlign, TextTransform, VerticalAlign, WhiteSpace, WordBreak, WritingMode,
};
use ebook_layout::{Layout, LayoutEnv, NoImageSizes, PageConstraint, StubMeasurer};

fn node(kind: NodeKind, style: ComputedStyle, parent: Option<u32>) -> Node {
    Node {
        kind,
        style,
        parent,
        first_child: 0,
        child_len: 0,
        src: None,
        link: None,
    }
}

/// root -> <p> -> text。节点下标：0 root，1 p，2 text。
fn base_doc() -> StyledDoc {
    let mut doc = StyledDoc::default();
    doc.meta.spine = 1;
    doc.meta.title = Some(String::from("demo"));
    doc.meta.lang = Some(String::from("en"));
    let root = doc.push_node(node(
        NodeKind::Element(Tag::Root),
        ComputedStyle::default(),
        None,
    ));
    doc.root = root;
    let block = doc.push_node(node(
        NodeKind::Element(Tag::P),
        ComputedStyle::default(),
        Some(root),
    ));
    let start = doc.text.len() as u32;
    doc.text.push_str("winter road lantern");
    let style = ComputedStyle::inline(
        FontSpec {
            font: FontId(0),
            size: 16.0,
            weight: 400,
            ..Default::default()
        },
        Rgba8::BLACK,
        Dir::Ltr,
    );
    let text = doc.push_node(node(
        NodeKind::Text(Span::new(start, doc.text.len() as u32 - start)),
        style,
        Some(block),
    ));
    doc.set_kids(block, &[text]);
    doc.set_kids(root, &[block]);
    doc.images.push(ImageMeta::default());
    doc
}

fn base_fonts() -> FontSet {
    let mut fonts = FontSet::default();
    fonts.push(
        FontDesc {
            family: String::from("Test Serif"),
            weight: 400,
            style: FontStyle::Normal,
        },
        (0..80u8).collect(),
    );
    fonts
}

fn constraint() -> PageConstraint {
    PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(24.0))
}

fn key(doc: &StyledDoc, fonts: &FontSet, c: &PageConstraint, rev: u32) -> CacheKey {
    fingerprint(doc, Some(fonts), c, rev)
}

#[test]
fn same_input_same_key() {
    let doc = base_doc();
    let fonts = base_fonts();
    let c = constraint();
    assert_eq!(key(&doc, &fonts, &c, 3), key(&doc, &fonts, &c, 3));
    assert_eq!(
        key(&doc, &fonts, &c, 3).value(),
        key(&doc, &fonts, &c, 3).value()
    );
    // 字体摘要自身也必须是确定的。
    assert_eq!(font_fingerprint(&fonts), font_fingerprint(&fonts));
}

#[test]
fn every_hashed_field_changes_the_key() {
    let doc = base_doc();
    let fonts = base_fonts();
    let c = constraint();
    let expected = key(&doc, &fonts, &c, 0);

    type Mutation = Box<dyn Fn(&mut StyledDoc)>;
    #[allow(clippy::type_complexity)]
    let mutations: Vec<(&str, Mutation)> = vec![
        (
            "display",
            Box::new(|d| d.node_mut(1).style.display = Display::InlineBlock),
        ),
        (
            "width",
            Box::new(|d| d.node_mut(1).style.width = Len::Px(123.0)),
        ),
        (
            "height",
            Box::new(|d| d.node_mut(1).style.height = Len::Pct(50.0)),
        ),
        (
            "margin",
            Box::new(|d| d.node_mut(1).style.margin.top = Len::Em(1.0)),
        ),
        (
            "padding",
            Box::new(|d| d.node_mut(1).style.padding.left = Len::Px(3.0)),
        ),
        (
            "border width",
            Box::new(|d| d.node_mut(1).style.border.width.top = Len::Px(2.0)),
        ),
        (
            "border color",
            Box::new(|d| d.node_mut(1).style.border.color.g = 9),
        ),
        (
            "font id",
            Box::new(|d| d.node_mut(2).style.font.font = FontId(2)),
        ),
        (
            "font size",
            Box::new(|d| d.node_mut(2).style.font.size = 17.5),
        ),
        (
            "font weight",
            Box::new(|d| d.node_mut(2).style.font.weight = 700),
        ),
        (
            "font style",
            Box::new(|d| d.node_mut(2).style.font.style = FontStyle::Italic),
        ),
        (
            "font family",
            Box::new(|d| {
                d.node_mut(2).style.font.families.push("Other");
            }),
        ),
        (
            "line height",
            Box::new(|d| d.node_mut(1).style.line_height = LineHeight::Mul(1.4)),
        ),
        (
            "text align",
            Box::new(|d| d.node_mut(1).style.text.align = TextAlign::Justify),
        ),
        (
            "text indent",
            Box::new(|d| d.node_mut(1).style.text.indent = Len::Px(8.0)),
        ),
        (
            "letter spacing",
            Box::new(|d| d.node_mut(1).style.text.letter_spacing = 0.5),
        ),
        (
            "word spacing",
            Box::new(|d| d.node_mut(1).style.text.word_spacing = 0.5),
        ),
        (
            "white space",
            Box::new(|d| d.node_mut(1).style.text.white_space = WhiteSpace::Pre),
        ),
        (
            "word break",
            Box::new(|d| d.node_mut(1).style.text.word_break = WordBreak::BreakAll),
        ),
        (
            "hyphens",
            Box::new(|d| d.node_mut(1).style.text.hyphens = true),
        ),
        (
            "transform",
            Box::new(|d| d.node_mut(1).style.text.transform = TextTransform::Uppercase),
        ),
        (
            "vertical align",
            Box::new(|d| d.node_mut(1).style.text.vertical_align = VerticalAlign::Super),
        ),
        ("color", Box::new(|d| d.node_mut(1).style.color.r = 7)),
        (
            "background",
            Box::new(|d| d.node_mut(1).style.background = Some(Rgba8::WHITE)),
        ),
        (
            "writing mode",
            Box::new(|d| d.node_mut(1).style.writing_mode = WritingMode::VerticalRl),
        ),
        ("dir", Box::new(|d| d.node_mut(1).style.dir = Dir::Rtl)),
        (
            "break before",
            Box::new(|d| d.node_mut(1).style.brk.before = ebook_ir::Break::Always),
        ),
        ("orphans", Box::new(|d| d.node_mut(1).style.brk.orphans = 3)),
        ("widows", Box::new(|d| d.node_mut(1).style.brk.widows = 3)),
        (
            "list",
            Box::new(|d| {
                d.node_mut(1).style.list = Some(ListStyle {
                    ordered: true,
                    start: 2,
                })
            }),
        ),
        (
            "link",
            Box::new(|d| d.node_mut(2).link = Some(String::from("ch2.xhtml"))),
        ),
        (
            "anchor",
            Box::new(|d| {
                d.node_mut(2).src = Some(Anchor {
                    spine: 1,
                    path: vec![0, 1],
                    offset: 4,
                })
            }),
        ),
        (
            "node kind",
            Box::new(|d| d.node_mut(2).kind = NodeKind::Text(Span::new(1, 4))),
        ),
        (
            "node style",
            Box::new(|d| d.node_mut(2).style.font.weight = 500),
        ),
        ("parent", Box::new(|d| d.node_mut(2).parent = Some(0))),
        ("text byte", Box::new(|d| d.text.replace_range(0..1, "W"))),
        (
            "text length",
            Box::new(|d| {
                d.text.push('!');
            }),
        ),
        (
            "children",
            Box::new(|d| {
                d.children.clear();
            }),
        ),
        (
            "node count",
            Box::new(|d| {
                d.push_node(node(
                    NodeKind::Element(Tag::Div),
                    ComputedStyle::default(),
                    Some(0),
                ));
            }),
        ),
        ("spine", Box::new(|d| d.meta.spine = 9)),
        (
            "title",
            Box::new(|d| d.meta.title = Some(String::from("other"))),
        ),
        ("language", Box::new(|d| d.meta.lang = None)),
        ("meta dir", Box::new(|d| d.meta.dir = Dir::Rtl)),
        (
            "meta writing mode",
            Box::new(|d| d.meta.writing_mode = WritingMode::VerticalLr),
        ),
        ("root", Box::new(|d| d.root = 1)),
        (
            "image meta",
            Box::new(|d| d.images[0].size = Some(SizeF::new(64.0, 32.0))),
        ),
        (
            "image alt",
            Box::new(|d| d.images[0].alt = Some(String::from("cover"))),
        ),
    ];

    for (name, mutate) in mutations {
        let mut changed = doc.clone();
        mutate(&mut changed);
        assert_ne!(
            key(&changed, &fonts, &c, 0),
            expected,
            "changing {name} did not change the cache key"
        );
    }
}

#[test]
fn constraint_and_policy_are_part_of_the_key() {
    let doc = base_doc();
    let fonts = base_fonts();
    let c = constraint();
    let expected = key(&doc, &fonts, &c, 0);

    let variants = [
        PageConstraint::new(601.0, 800.0).with_margin(EdgesF::all(24.0)),
        PageConstraint::new(600.0, 801.0).with_margin(EdgesF::all(24.0)),
        PageConstraint::new(600.0, 800.0).with_margin(EdgesF::all(25.0)),
        PageConstraint::new(600.0, 800.0).with_margin(EdgesF {
            top: 24.0,
            right: 24.0,
            bottom: 24.0,
            left: 25.0,
        }),
    ];
    for variant in variants {
        assert_ne!(key(&doc, &fonts, &variant, 0), expected);
    }
    assert_ne!(key(&doc, &fonts, &c, 1), expected);

    let mut c2 = constraint();
    c2.writing_mode = WritingMode::VerticalRl;
    assert_ne!(key(&doc, &fonts, &c2, 0), expected);
}

#[test]
fn font_identity_is_part_of_the_key() {
    let doc = base_doc();
    let fonts = base_fonts();
    let c = constraint();
    let expected = key(&doc, &fonts, &c, 0);

    let families = [
        {
            let mut f = fonts.clone();
            f.fonts[0].desc.family = String::from("Other");
            f
        },
        {
            let mut f = fonts.clone();
            f.fonts[0].desc.weight = 700;
            f
        },
        {
            let mut f = fonts.clone();
            f.fonts[0].desc.style = FontStyle::Italic;
            f
        },
        {
            let mut f = fonts.clone();
            f.fonts[0].bytes[3] ^= 0xff;
            f
        },
        {
            let mut f = fonts.clone();
            f.fonts[0].bytes.push(0);
            f
        },
        {
            let mut f = fonts.clone();
            f.push(FontDesc::default(), vec![1, 2, 3]);
            f
        },
    ];
    for variant in families {
        assert_ne!(
            key(&doc, &variant, &c, 0),
            expected,
            "font change did not change the key"
        );
    }

    // 超过前 64 字节的部分不参与哈希（这是有意的，避免重复哈希几十 MB）。
    let mut tail_only = fonts.clone();
    tail_only.fonts[0].bytes[79] ^= 0xff;
    assert_eq!(key(&doc, &tail_only, &c, 0), expected);
}

#[test]
fn fingerprint_does_not_hash_the_runtime_environment() {
    // 同一份输入、两个不同的 measurer 实例：键相同，因为键只由输入决定。
    let doc = base_doc();
    let fonts = base_fonts();
    let c = constraint();
    let env_a = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let env_b = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let a: LayoutResult = Layout::new().layout(&doc, &env_a, &c);
    let b: LayoutResult = Layout::new().layout(&doc, &env_b, &c);
    assert_eq!(
        ebook_ir::codec::to_bytes(&a).unwrap(),
        ebook_ir::codec::to_bytes(&b).unwrap()
    );
    assert_eq!(key(&doc, &fonts, &c, 0), key(&doc, &fonts, &c, 0));
}
