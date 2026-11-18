#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod css;
pub mod epub;
pub mod txt;
pub mod xhtml;
pub mod zip;

use alloc::string::String;
use alloc::vec::Vec;

use ebook_ir::StyledDoc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Book {
    pub title: Option<String>,
    pub lang: Option<String>,
    pub spine: Vec<SpineItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpineItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub linear: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemorySource {
    files: Vec<(String, Vec<u8>)>,
}

impl MemorySource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: &str, bytes: Vec<u8>) {
        self.files.push((String::from(name), bytes));
    }

    pub fn insert_text(&mut self, name: &str, text: &str) {
        self.insert(name, text.as_bytes().to_vec());
    }
}

pub trait ByteSource {
    fn read(&self, name: &str) -> Option<Vec<u8>>;
    fn names(&self) -> Vec<&str>;
    fn document(&self) -> Option<Vec<u8>> {
        None
    }
    fn first_name(&self) -> Option<&str> {
        self.names().first().copied()
    }
}

impl ByteSource for MemorySource {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.clone())
    }

    fn names(&self) -> Vec<&str> {
        self.files.iter().map(|(n, _)| n.as_str()).collect()
    }

    fn document(&self) -> Option<Vec<u8>> {
        match self.files.as_slice() {
            [(_, bytes)] => Some(bytes.clone()),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    No,
    Maybe,
    Yes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    Unsupported(&'static str),
    Missing(&'static str),
    Malformed(&'static str),
    OutOfRange,
}

pub trait FormatAdapter: Sync {
    fn name(&self) -> &'static str;
    fn probe(&self, src: &dyn ByteSource) -> Probe;
    fn open(&self, src: &dyn ByteSource) -> Result<Book, FormatError>;
    fn chapter(
        &self,
        src: &dyn ByteSource,
        book: &Book,
        index: usize,
    ) -> Result<StyledDoc, FormatError>;
}

static TXT: txt::Txt = txt::Txt;
static EPUB: epub::Epub = epub::Epub;
static ADAPTERS: [&dyn FormatAdapter; 2] = [&TXT, &EPUB];

pub fn adapters() -> &'static [&'static dyn FormatAdapter] {
    &ADAPTERS
}

pub struct Opened {
    adapter: &'static dyn FormatAdapter,
    pub book: Book,
}

impl Opened {
    pub fn adapter_name(&self) -> &'static str {
        self.adapter.name()
    }

    pub fn chapter(&self, src: &dyn ByteSource, index: usize) -> Result<StyledDoc, FormatError> {
        self.adapter.chapter(src, &self.book, index)
    }

    pub fn len(&self) -> usize {
        self.book.spine.len()
    }

    pub fn is_empty(&self) -> bool {
        self.book.spine.is_empty()
    }
}

pub fn open(src: &dyn ByteSource) -> Result<Opened, FormatError> {
    let mut chosen: Option<&'static dyn FormatAdapter> = None;
    let mut best = Probe::No;
    for adapter in adapters() {
        let probe = adapter.probe(src);
        if probe == Probe::Yes {
            return Ok(Opened { adapter: *adapter, book: adapter.open(src)? });
        }
        if probe == Probe::Maybe && best == Probe::No {
            best = probe;
            chosen = Some(*adapter);
        }
    }
    match chosen {
        Some(adapter) => Ok(Opened { adapter, book: adapter.open(src)? }),
        None => Err(FormatError::Unsupported("no adapter")),
    }
}
