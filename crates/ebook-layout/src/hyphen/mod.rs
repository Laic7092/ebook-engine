//! 断字（hyphenation）：可插接口 + 内置英文 TeX 模式。
//!
//! 排版层只定义 [`Hyphenator`] 接口，词典由宿主通过
//! [`TextMeasurer::hyphenator`](crate::TextMeasurer::hyphenator) 注入；核心不捆绑
//! "默认词典"，因为换词典会改变分页，属于政策而不是正确性。
//!
//! 内置的 `English`（`hyphen-en` feature）是 ushyphmax 模式（hyph-utf8 /
//! `~/layx/render/hyphen/en`），用 Liang 算法求断点，数据由
//! `data/gen_hyphen.py` 编成二进制后用 `include_bytes!` 带进来，不启动时解析、
//! 不持全局状态。默认不编译这份数据。

use alloc::vec::Vec;

#[cfg(feature = "hyphen-en")]
pub mod en;

#[cfg(feature = "hyphen-en")]
pub use en::English;

pub trait Hyphenator {
    /// 把 `word` 内允许插入连字符的位置写进 `out`：升序、**字节偏移**、
    /// 落在 `(0, word.len())` 内。非字母词直接返回空。
    ///
    /// 这里用 `out` 而不是返回 `&[u16]`：`&self` 方法无法把一次计算的结果
    /// 借着返回，除非引入内部可变性（`RefCell`/`thread_local`），而排版层
    /// 明确禁止全局可变状态。语义与"返回断点切片"完全一致。
    fn breaks(&self, word: &str, out: &mut Vec<u16>);

    /// 词典名，用于缓存键 / 诊断。
    fn name(&self) -> &str;
}

impl<H: Hyphenator + ?Sized> Hyphenator for &H {
    fn breaks(&self, word: &str, out: &mut Vec<u16>) {
        (*self).breaks(word, out)
    }

    fn name(&self) -> &str {
        (*self).name()
    }
}
