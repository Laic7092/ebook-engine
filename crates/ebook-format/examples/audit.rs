//! 逐章审计一个 EPUB 目录：打开每本书、解析每个 spine 章节，打印错误/空章与
//! 诊断。用于回归公开语料和用户反馈的书，不参与运行时。
//!
//! ```text
//! cargo run -q -p ebook-format --example audit -- /path/to/*.epub
//! ```
//!
//! 退出码：全部章节可解析为 0，否则为 1。

use std::fs;
use std::process::ExitCode;

use ebook_format::epub::EpubReader;

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: audit <book.epub>...");
        return ExitCode::from(2);
    }

    let mut failed = false;
    for path in paths {
        println!("=== {path}");
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(error) => {
                println!("  READ ERR {error}");
                failed = true;
                continue;
            }
        };
        let reader = match EpubReader::open(&data) {
            Ok(reader) => reader,
            Err(error) => {
                println!("  OPEN ERR {error:?}");
                failed = true;
                continue;
            }
        };

        let book = reader.book();
        println!(
            "  title={:?} spine={} toc={} issues={}",
            book.title,
            book.spine.len(),
            book.toc.len(),
            book.issues.len()
        );
        for issue in &book.issues {
            println!(
                "   issue {:?} {:?} {}",
                issue.code, issue.href, issue.detail
            );
        }

        let (mut ok, mut empty, mut errs) = (0usize, 0usize, 0usize);
        for (index, entry) in book.spine.iter().enumerate() {
            let href = &entry.href;
            match reader.chapter(index) {
                Ok(doc) if doc.text.is_empty() => {
                    empty += 1;
                    if empty <= 5 {
                        println!(
                            "   spine {index} EMPTY href={href} imgs={}",
                            doc.images.len()
                        );
                    }
                }
                Ok(_) => ok += 1,
                Err(error) => {
                    errs += 1;
                    if errs <= 5 {
                        println!("   spine {index} ERR {error:?} href={href}");
                    }
                }
            }
        }
        // 纯图片章节允许为空；真正的问题只有解析错误。
        println!("  summary ok={ok} empty={empty} err={errs}");
        failed |= errs > 0;
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
