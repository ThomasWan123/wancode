//! PDF text extraction for Work documents.
//!
//! PDFium runs only inside the existing crash-contained parse worker. The
//! parent process verifies the staged file hash before spawning that worker,
//! and this module applies page/text limits before returning any blocks.

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

use crate::work_anchor::utf16_len;
use crate::work_blocks::WorkBlock;

#[derive(Debug, Clone, Copy)]
pub struct PdfLimits {
    pub max_pages: usize,
    pub max_page_utf16: usize,
    pub max_total_utf16: usize,
}

impl Default for PdfLimits {
    fn default() -> Self {
        Self {
            max_pages: 500,
            max_page_utf16: 256 * 1024,
            max_total_utf16: 2 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    PdfiumUnavailable(String),
    Load(String),
    TooManyPages {
        pages: usize,
        cap: usize,
    },
    PageText {
        page: usize,
        reason: String,
    },
    PageTextTooLarge {
        page: usize,
        units: usize,
        cap: usize,
    },
    TotalTextTooLarge {
        units: usize,
        cap: usize,
    },
    NoExtractableText,
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PdfiumUnavailable(reason) => write!(f, "PDFium 不可用：{reason}"),
            Self::Load(reason) => write!(f, "PDF 加载失败：{reason}"),
            Self::TooManyPages { pages, cap } => write!(f, "PDF 共 {pages} 页，超过上限 {cap}"),
            Self::PageText { page, reason } => write!(f, "PDF 第 {page} 页文本抽取失败：{reason}"),
            Self::PageTextTooLarge { page, units, cap } => {
                write!(
                    f,
                    "PDF 第 {page} 页文本 {units} UTF-16 单元，超过上限 {cap}"
                )
            }
            Self::TotalTextTooLarge { units, cap } => {
                write!(f, "PDF 文本累计 {units} UTF-16 单元，超过上限 {cap}")
            }
            Self::NoExtractableText => write!(f, "PDF 没有可提取文字（扫描件 OCR 尚未支持）"),
        }
    }
}

impl std::error::Error for PdfError {}

/// Parse a PDF into one stable block per non-empty page.
pub fn parse_pdf(path: &Path, limits: PdfLimits) -> Result<Vec<WorkBlock>, PdfError> {
    let library = locate_pdfium().ok_or_else(|| {
        PdfError::PdfiumUnavailable(
            "找不到经过供应链锁校验的 pdfium.dll；请重新安装或修复应用".into(),
        )
    })?;
    let bindings = Pdfium::bind_to_library(&library)
        .map_err(|e| PdfError::PdfiumUnavailable(format!("{}：{e}", library.display())))?;
    let pdfium = Pdfium::new(bindings);
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| PdfError::Load(e.to_string()))?;
    let pages = document.pages().len() as usize;
    if pages > limits.max_pages {
        return Err(PdfError::TooManyPages {
            pages,
            cap: limits.max_pages,
        });
    }

    let mut blocks = Vec::new();
    let mut total = 0usize;
    for (index, page) in document.pages().iter().enumerate() {
        let raw = page
            .text()
            .map_err(|e| PdfError::PageText {
                page: index + 1,
                reason: e.to_string(),
            })?
            .all()
            .replace('\0', "")
            .trim()
            .to_string();
        if raw.is_empty() {
            continue;
        }
        let units = utf16_len(&raw);
        if units > limits.max_page_utf16 {
            return Err(PdfError::PageTextTooLarge {
                page: index + 1,
                units,
                cap: limits.max_page_utf16,
            });
        }
        total = total.saturating_add(units);
        if total > limits.max_total_utf16 {
            return Err(PdfError::TotalTextTooLarge {
                units: total,
                cap: limits.max_total_utf16,
            });
        }
        blocks.push(WorkBlock {
            path: format!("page[{}]/chunk[0]", index + 1),
            raw,
            runs: vec![[0, units]],
        });
    }
    if blocks.is_empty() {
        return Err(PdfError::NoExtractableText);
    }
    Ok(blocks)
}

fn locate_pdfium() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(Pdfium::pdfium_platform_library_name()));
            candidates.push(
                dir.join("resources")
                    .join(Pdfium::pdfium_platform_library_name()),
            );
        }
    }
    #[cfg(debug_assertions)]
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("vendor")
            .join("pdfium-runtime")
            .join("bin")
            .join(Pdfium::pdfium_platform_library_name()),
    );
    candidates.into_iter().find(|candidate| candidate.is_file())
}
