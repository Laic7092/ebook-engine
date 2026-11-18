//! 仓库里每个样书都要能打开、读元数据、逐章解析。

use std::path::PathBuf;

use ebook_format::{open, MemorySource};

#[test]
fn every_fixture_opens_and_reads_every_chapter() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut checked = 0usize;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("epub"))
        .collect();
    entries.sort();

    for path in entries {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let bytes = std::fs::read(&path).expect("read fixture");
        let mut src = MemorySource::new();
        src.insert(&name, bytes);
        let opened = open(&src).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert!(!opened.is_empty(), "{name}: empty spine");
        assert!(
            opened
                .book
                .title
                .as_deref()
                .is_some_and(|title| !title.is_empty()),
            "{name}: missing title"
        );
        for index in 0..opened.len() {
            let doc = opened
                .chapter(&src, index)
                .unwrap_or_else(|error| panic!("{name} chapter {index}: {error:?}"));
            assert!(!doc.text.is_empty(), "{name} chapter {index}: empty text");
            for id in doc.walk() {
                assert!(
                    doc.node(id).src.is_some(),
                    "{name} chapter {index}: node {id} has no anchor"
                );
            }
        }
        checked += 1;
    }

    assert!(checked >= 5, "only checked {checked} fixtures");
}
