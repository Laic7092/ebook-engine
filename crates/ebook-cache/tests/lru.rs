use ebook_cache::{CacheStats, LayoutCache};
use ebook_ir::{
    codec, ComputedStyle, Dir, EdgesF, FontDesc, FontId, FontSet, FontSpec, Node, NodeKind, Rgba8,
    Span, StyledDoc, Tag,
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

fn doc(words: &str) -> StyledDoc {
    let mut doc = StyledDoc::default();
    let root = doc.push_node(node(
        NodeKind::Element(Tag::Root),
        ComputedStyle::default(),
        None,
    ));
    doc.root = root;
    let start = doc.text.len() as u32;
    doc.text.push_str(words);
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
        Some(root),
    ));
    doc.set_kids(root, &[text]);
    doc
}

fn constraint() -> PageConstraint {
    PageConstraint::new(240.0, 160.0).with_margin(EdgesF::all(10.0))
}

fn fresh(doc: &StyledDoc, c: &PageConstraint) -> Vec<u8> {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    codec::to_bytes(&Layout::new().layout(doc, &env, c)).expect("encode")
}

#[test]
fn misses_hits_and_lru_evictions_are_counted() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let c = constraint();
    let a = doc("winter road lantern ice harbor");
    let b = doc("brass letter shelf stove ridge");
    let d = doc("wind and snow and a long quiet road");

    let mut cache = LayoutCache::new(2);
    assert_eq!(cache.capacity(), 2);

    let _ = cache.layout(&a, &env, &c);
    let _ = cache.layout(&b, &env, &c);
    assert_eq!(
        cache.stats(),
        CacheStats {
            hits: 0,
            misses: 2,
            evictions: 0
        }
    );
    assert_eq!(cache.len(), 2);

    // 命中 A，让它变成最近使用；之后 C 进来应该淘汰 B。
    let hit_bytes = codec::to_bytes(cache.layout(&a, &env, &c)).expect("encode");
    assert_eq!(
        hit_bytes,
        fresh(&a, &c),
        "cache hit returned different bytes"
    );
    assert_eq!(cache.stats().hits, 1);

    // B 是最久未用的，被 C 淘汰。
    let _ = cache.layout(&d, &env, &c);
    assert_eq!(cache.stats().misses, 3);
    assert_eq!(cache.stats().evictions, 1);
    assert_eq!(cache.len(), 2);

    // A 命中后变成最近使用，C 变成 LRU；B 再进来淘汰 C，而不是 A。
    let again = codec::to_bytes(cache.layout(&a, &env, &c)).expect("encode");
    assert_eq!(again, hit_bytes);
    assert_eq!(cache.stats().hits, 2);

    let _ = cache.layout(&b, &env, &c);
    assert_eq!(cache.stats().misses, 4);
    assert_eq!(cache.stats().evictions, 2);
    assert_eq!(cache.len(), 2);

    // A 仍在缓存里。
    let again = codec::to_bytes(cache.layout(&a, &env, &c)).expect("encode");
    assert_eq!(again, hit_bytes);
    assert_eq!(cache.stats().hits, 3);
}

#[test]
fn a_hit_does_not_recompute_and_stays_byte_identical() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let c = constraint();
    let doc = doc("winter road lantern ice harbor brass letter shelf stove ridge wind");
    let expected = fresh(&doc, &c);

    let mut cache = LayoutCache::new(4);
    let first = codec::to_bytes(cache.layout(&doc, &env, &c)).expect("encode");
    let second = codec::to_bytes(cache.layout(&doc, &env, &c)).expect("encode");
    let third = codec::to_bytes(cache.layout(&doc, &env, &c)).expect("encode");
    assert_eq!(first, expected);
    assert_eq!(second, expected);
    assert_eq!(third, expected);
    assert_eq!(cache.stats().misses, 1);
    assert_eq!(cache.stats().hits, 2);
    assert_eq!(cache.stats().evictions, 0);
}

#[test]
fn policy_revision_and_fonts_invalidate_by_key() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let c = constraint();
    let doc = doc("winter road lantern ice harbor");

    let mut cache = LayoutCache::new(2).with_policy_rev(1);
    let _ = cache.layout(&doc, &env, &c);
    let _ = cache.layout(&doc, &env, &c);
    assert_eq!(cache.stats().misses, 1);
    assert_eq!(cache.stats().hits, 1);

    cache.set_policy_rev(2);
    let _ = cache.layout(&doc, &env, &c);
    assert_eq!(cache.stats().misses, 2);

    let mut fonts = FontSet::default();
    fonts.push(
        FontDesc {
            family: String::from("Serif"),
            weight: 400,
            style: ebook_ir::FontStyle::Normal,
        },
        vec![1, 2, 3, 4],
    );
    cache.set_fonts(&fonts);
    let _ = cache.layout(&doc, &env, &c);
    assert_eq!(cache.stats().misses, 3);

    // 同一份字体再次登记不改变键。
    cache.set_fonts(&fonts);
    let _ = cache.layout(&doc, &env, &c);
    assert_eq!(cache.stats().misses, 3);
    assert_eq!(cache.stats().hits, 2);
}

#[test]
fn cache_capacity_one_still_returns_a_borrowed_result() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let c = constraint();
    let a = doc("winter road");
    let b = doc("brass letter");
    let mut cache = LayoutCache::new(0);
    assert_eq!(cache.capacity(), 1);
    let _ = cache.layout(&a, &env, &c);
    let _ = cache.layout(&b, &env, &c);
    assert_eq!(cache.stats().misses, 2);
    assert_eq!(cache.stats().evictions, 1);
    assert_eq!(cache.len(), 1);
    assert_eq!(
        codec::to_bytes(cache.layout(&b, &env, &c)).unwrap(),
        fresh(&b, &c)
    );
}

#[test]
fn different_documents_never_share_an_entry() {
    let env = LayoutEnv {
        measurer: &StubMeasurer,
        resources: &NoImageSizes,
    };
    let c = constraint();
    let a = doc("winter road lantern");
    let b = doc("winter road lantern "); // 多一个空格：布局输出可能相同，键必须不同
    let a_bytes = fresh(&a, &c);
    let b_bytes = fresh(&b, &c);
    let mut cache = LayoutCache::new(4);
    assert_eq!(
        codec::to_bytes(cache.layout(&a, &env, &c)).unwrap(),
        a_bytes
    );
    assert_eq!(
        codec::to_bytes(cache.layout(&b, &env, &c)).unwrap(),
        b_bytes
    );
    assert_eq!(cache.stats().misses, 2);
    assert_eq!(cache.stats().hits, 0);
}
