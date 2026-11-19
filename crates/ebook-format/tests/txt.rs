use ebook_format::txt::TxtReader;

#[test]
fn headings_split_chapters_and_fill_toc() {
    let text = "第一章 雪夜\n\n那年冬天。\n\n第二章 旧书\n\n门是自己开的。\n";
    let reader = TxtReader::from_text(text, Some("story.txt"));
    let book = reader.book();

    assert_eq!(book.title.as_deref(), Some("story"));
    assert_eq!(book.spine.len(), 2);
    assert_eq!(book.toc.len(), 2);
    assert_eq!(book.toc[0].label, "第一章 雪夜");
    assert_eq!(book.toc[0].spine, 0);
    assert_eq!(book.toc[1].label, "第二章 旧书");
    assert_eq!(book.toc[1].spine, 1);

    let first = reader.chapter(0).unwrap();
    let second = reader.chapter(1).unwrap();
    assert!(first.text.contains("那年冬天"));
    assert!(second.text.contains("门是自己开的"));
    assert_eq!(second.meta.spine, 1);
    // 文本节点的锚点必须改写到本章的 spine，否则跳转/进度会落回第 0 章。
    assert!(second
        .nodes
        .iter()
        .filter_map(|node| node.src.as_ref())
        .all(|anchor| anchor.spine == 1));
}

#[test]
fn text_without_headings_is_still_readable() {
    let reader = TxtReader::from_text("hello world\n\nsecond paragraph\n", Some("notes.txt"));
    let book = reader.book();
    assert_eq!(book.spine.len(), 1);
    assert_eq!(book.toc[0].label, "hello world");
    let doc = reader.chapter(0).unwrap();
    assert!(doc.text.contains("hello world"));
    assert!(doc.text.contains("second paragraph"));
}

#[test]
fn very_long_text_is_chunked_on_line_boundaries() {
    // 128 KiB 兜底切分；260 KiB 至少应该切成两章。
    let line = "这是一行足够长的正文，用来验证超长 TXT 的兜底分章。\n";
    let mut text = String::new();
    while text.len() < 260 * 1024 {
        text.push_str(line);
    }
    let reader = TxtReader::from_text(&text, None);
    assert!(
        reader.book().spine.len() >= 2,
        "{}",
        reader.book().spine.len()
    );
    for index in 0..reader.book().spine.len() {
        assert!(reader.chapter(index).is_ok());
    }
}

#[test]
fn empty_text_still_opens() {
    let reader = TxtReader::from_text("", Some("empty.txt"));
    assert_eq!(reader.book().spine.len(), 1);
    assert_eq!(reader.book().title.as_deref(), Some("empty"));
    assert!(reader.chapter(0).is_ok());
}
