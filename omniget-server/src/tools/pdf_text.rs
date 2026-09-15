//! Pure-Rust PDF Text Extraction Tool via `lopdf`.
//!
//! Features:
//! - In-process execution with zero external C/C++ libraries (unlike PDFium).
//! - Accepts local filesystem paths or remote HTTP/HTTPS URLs.
//! - Downloads remote PDFs into memory using `reqwest`.
//! - Supports flexible page range filters ("1-3", "1, 5", "all", "*").
//! - Reports total processed page counts and structured plain text.
//! - Immediately releases file handles to prevent file lock issues during cleanup.

use anyhow::{anyhow, Result};
use lopdf::Document;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// Arguments for the `pdf_text` extraction tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfTextArgs {
    /// Local filesystem path to the PDF document.
    pub path: Option<String>,
    /// Alternative parameter name for remote URL.
    pub url: Option<String>,
    /// Optional page range filter: e.g. "1-3", "1, 3, 5", "all". Defaults to all pages.
    pub pages: Option<String>,
}

/// Structured response returned by `pdf_text`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PdfExtractResult {
    /// Extracted plain text content across selected pages.
    pub text: String,
    /// Number of pages successfully processed.
    pub pages: usize,
}

/// Parse a flexible page range string into 1-based page numbers.
///
/// Supported formats:
/// - `None`, `""`, `"all"`, `"*"` -> all pages `1..=total`
/// - Single numbers: `"1"`, `"3"`
/// - Ranges: `"1-3"`, `"2-4"`
/// - Comma/space separated: `"1, 2, 5"`, `"1-2, 4"`
/// - Reverse ranges: `"3-1"` -> `[3, 2, 1]`
pub fn parse_page_ranges(spec: Option<&str>, total: usize) -> Result<Vec<u32>> {
    let spec = match spec {
        None => return Ok((1..=(total as u32)).collect()),
        Some(s) => s.trim(),
    };

    if spec.is_empty() || spec.eq_ignore_ascii_case("all") || spec == "*" {
        return Ok((1..=(total as u32)).collect());
    }

    let mut pages = Vec::new();
    for part in spec.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()) {
        if let Some((start_str, end_str)) = part.split_once('-') {
            let start_trimmed = start_str.trim();
            let end_trimmed = end_str.trim();
            if start_trimmed.is_empty() || end_trimmed.is_empty() {
                anyhow::bail!("Invalid page range syntax: '{}'", part);
            }
            let start: usize = start_trimmed
                .parse()
                .map_err(|_| anyhow!("Invalid start page in range '{}'", part))?;
            let end: usize = end_trimmed
                .parse()
                .map_err(|_| anyhow!("Invalid end page in range '{}'", part))?;
            if start == 0 || end == 0 {
                anyhow::bail!("Page numbers must be 1-based (got 0 in '{}')", part);
            }
            if start > total || end > total {
                anyhow::bail!(
                    "Page range '{}' out of bounds for document with {} pages",
                    part,
                    total
                );
            }
            if start <= end {
                pages.extend((start as u32)..=(end as u32));
            } else {
                pages.extend(((end as u32)..=(start as u32)).rev());
            }
        } else {
            let page: usize = part
                .parse()
                .map_err(|_| anyhow!("Invalid page number: '{}'", part))?;
            if page == 0 {
                anyhow::bail!("Page numbers must be 1-based (got 0 in '{}')", part);
            }
            if page > total {
                anyhow::bail!("Page {} out of bounds for document with {} pages", page, total);
            }
            pages.push(page as u32);
        }
    }

    if pages.is_empty() {
        anyhow::bail!("No pages selected by range '{}'", spec);
    }

    Ok(pages)
}

/// Download or read PDF binary data into memory.
async fn load_pdf_bytes(target: &str) -> Result<Vec<u8>> {
    if target.starts_with("http://") || target.starts_with("https://") {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("OmniGet/0.1 (PDF Document Extractor)")
            .build()
            .unwrap_or_default();

        let resp = client
            .get(target)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch remote PDF from {}: {}", target, e))?;

        if !resp.status().is_success() {
            anyhow::bail!("Remote PDF fetch failed with HTTP status {}", resp.status());
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| anyhow!("Failed to read PDF response body from {}: {}", target, e))?;
        Ok(bytes.to_vec())
    } else {
        let path = Path::new(target);
        if !path.exists() {
            anyhow::bail!("PDF file does not exist: {}", target);
        }

        // tokio::fs::read opens, buffers into memory, and immediately closes file descriptor,
        // preventing Windows/Unix file locking issues on temporary files.
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| anyhow!("Failed to read PDF file at {}: {}", target, e))?;
        Ok(bytes)
    }
}

/// Extract plain text content and processed page count from a PDF.
pub async fn extract_pdf_text(args: PdfTextArgs) -> Result<PdfExtractResult> {
    let target = args
        .path
        .as_deref()
        .or(args.url.as_deref())
        .map(str::trim)
        .unwrap_or("");

    if target.is_empty() {
        anyhow::bail!("Missing required parameter: path or url");
    }

    let bytes = load_pdf_bytes(target).await?;

    // Load PDF in memory via pure-Rust lopdf parser (zero C dependencies)
    let doc = Document::load_mem(&bytes)
        .map_err(|e| anyhow!("Failed to parse PDF document: {}", e))?;

    let pages_map = doc.get_pages();
    let total_pages = pages_map.len();

    if total_pages == 0 {
        return Ok(PdfExtractResult {
            text: String::new(),
            pages: 0,
        });
    }

    let target_pages = parse_page_ranges(args.pages.as_deref(), total_pages)?;

    let mut page_texts = Vec::new();
    for &page_num in &target_pages {
        let text = match doc.extract_text(&[page_num]) {
            Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => {
                if let Some(&page_id) = pages_map.get(&page_num) {
                    extract_page_text_fallback(&doc, page_id)
                } else {
                    String::new()
                }
            }
        };
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            page_texts.push(trimmed.to_string());
        }
    }

    let full_text = page_texts.join("\n\n");
    let processed_count = target_pages.len();

    Ok(PdfExtractResult {
        text: full_text,
        pages: processed_count,
    })
}

/// Fallback text extractor directly parsing content stream operations
/// for documents lacking font resources in their page dictionary.
fn extract_page_text_fallback(doc: &Document, page_id: lopdf::ObjectId) -> String {
    let content_data = doc.get_page_content(page_id);
    if let Ok(content) = lopdf::content::Content::decode(&content_data) {
        let mut text = String::new();
        for op in &content.operations {
            match op.operator.as_str() {
                "Tj" | "'" => {
                    for operand in &op.operands {
                        if let lopdf::Object::String(bytes, _) = operand {
                            text.push_str(&String::from_utf8_lossy(bytes));
                        }
                    }
                }
                "\"" => {
                    if let Some(lopdf::Object::String(bytes, _)) = op.operands.get(2) {
                        text.push_str(&String::from_utf8_lossy(bytes));
                    }
                }
                "TJ" => {
                    for operand in &op.operands {
                        if let lopdf::Object::Array(arr) = operand {
                            for item in arr {
                                if let lopdf::Object::String(bytes, _) = item {
                                    text.push_str(&String::from_utf8_lossy(bytes));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        text
    } else {
        String::new()
    }
}

/// JSON-RPC `tools/call` handler for `pdf_text`.
pub async fn call_pdf_text(arguments: Value) -> Result<Value, anyhow::Error> {
    let args: PdfTextArgs = serde_json::from_value(arguments)
        .map_err(|e| anyhow!("Invalid arguments for pdf_text: {}", e))?;
    match extract_pdf_text(args).await {
        Ok(res) => Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": res.text
                }
            ],
            "pages": res.pages,
            "isError": false
        })),
        Err(e) => Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": e.to_string()
                }
            ],
            "isError": true
        })),
    }
}
