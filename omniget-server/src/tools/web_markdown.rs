//! Universal Web-to-Markdown scraper and converter via reqwest and htmd.

use anyhow::{anyhow, Result};
use htmd::options::{
    BrStyle, BulletListMarker, CodeBlockFence, CodeBlockStyle, HeadingStyle, HrStyle,
    LinkReferenceStyle, LinkStyle, Options as MdOptions, TranslationMode,
};
use htmd::{element_handler::HandlerResult, Element, HtmlToMarkdown};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;
use url::Url;

/// Result of a web-to-markdown extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebMarkdownResult {
    pub title: String,
    pub markdown: String,
    pub url: String,
}

// ── Tracking Parameter Cleaner ───────────────────────────────────────────

const TRACKING_KEYS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "source",
    "ref",
    "referrer",
    "publication_id",
    "post_id",
    "isFreemail",
    "triedRedirect",
    "r",
    "showWelcomeOnShare",
    "gi",
    "sk",
    "fbclid",
    "gclid",
    "mc_eid",
    "_ga",
    "_gl",
];

/// Strips tracking parameters from a URL while preserving other query parameters and fragments.
pub fn clean_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let (query, frag) = match query.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (query, None),
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let key = p.split_once('=').map(|(k, _)| k).unwrap_or(p);
            !TRACKING_KEYS.iter().any(|t| t.eq_ignore_ascii_case(key))
        })
        .collect();
    let mut out = base.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

// ── Junk Elements & Noise Filtering ─────────────────────────────────────

const JUNK_CLASSES_AND_IDS: &[&str] = &[
    "subscription-widget",
    "subscribe-widget",
    "subscribe-footer",
    "button-wrapper",
    "share-dialog",
    "social-share",
    "post-ufi",
    "post-footer",
    "comments-page",
    "paywall",
    "pencraft-cta",
    "js-postMetaInline",
    "postActions",
    "recommendation",
    "advertisement",
    "ad-container",
    "ad-wrapper",
    "cookie-banner",
    "cookie-notice",
    "consent-banner",
    "popup-overlay",
    "modal-backdrop",
];

fn attr<'a>(el: &'a Element<'a>, name: &str) -> Option<String> {
    el.attrs
        .iter()
        .find(|a| a.name.local.as_ref() == name)
        .map(|a| a.value.to_string())
}

fn is_junk(class_or_id: &str) -> bool {
    let lower = class_or_id.to_lowercase();
    JUNK_CLASSES_AND_IDS
        .iter()
        .any(|j| lower.contains(&j.to_lowercase()))
}

// ── Title Extractor ──────────────────────────────────────────────────────

static OG_TITLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<meta\s+[^>]*?(?:property|name)=["'](?:og:title|twitter:title)["'][^>]*?content=["']([^"']+)["']"#)
        .expect("og title regex")
});

static OG_TITLE_REV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<meta\s+[^>]*?content=["']([^"']+)["'][^>]*?(?:property|name)=["'](?:og:title|twitter:title)["']"#)
        .expect("og title rev regex")
});

static TITLE_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<title[^>]*>(.*?)</title>"#).expect("title tag regex")
});

static H1_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<h1[^>]*>(.*?)</h1>"#).expect("h1 tag regex")
});

fn unescape_html(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

pub fn extract_title(html: &str, fallback_url: &str) -> String {
    if let Some(c) = OG_TITLE_RE.captures(html).or_else(|| OG_TITLE_REV_RE.captures(html)) {
        let t = unescape_html(c[1].trim());
        if !t.is_empty() {
            return t;
        }
    }
    if let Some(c) = TITLE_TAG_RE.captures(html) {
        let t = unescape_html(c[1].trim());
        if !t.is_empty() {
            return t;
        }
    }
    if let Some(c) = H1_TAG_RE.captures(html) {
        let raw = strip_html_tags(&c[1]);
        let t = unescape_html(raw.trim());
        if !t.is_empty() {
            return t;
        }
    }
    fallback_url.rsplit('/').next().unwrap_or("Web Page").to_string()
}

// ── HTML Table Normalization ─────────────────────────────────────────────

static TABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<table[^>]*>.*?</table>"#).expect("table regex")
});

static FIRST_TR_TD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)(<tr[^>]*>)(.*?)</tr>"#).expect("first tr td regex")
});

/// Promotes the first row's `<td>` elements to `<th>` if the table lacks explicit `<th>` cells,
/// ensuring htmd in Pure mode triggers its Markdown pipe table generator.
fn normalize_tables_for_htmd(html: &str) -> String {
    TABLE_RE.replace_all(html, |caps: &regex::Captures| {
        let table = &caps[0];
        if table.to_lowercase().contains("<th") {
            return table.to_string();
        }
        FIRST_TR_TD_RE.replace(table, |tr_caps: &regex::Captures| {
            let tr_open = &tr_caps[1];
            let cells = &tr_caps[2];
            let promoted = cells
                .replace("<td", "<th")
                .replace("</td>", "</th>")
                .replace("<TD", "<TH")
                .replace("</TD>", "</th>");
            format!("{}{}</tr>", tr_open, promoted)
        }).to_string()
    }).to_string()
}

// ── Converter Construction ───────────────────────────────────────────────

static CONVERTER: LazyLock<HtmlToMarkdown> = LazyLock::new(build_converter);

fn build_converter() -> HtmlToMarkdown {
    let options = MdOptions {
        heading_style: HeadingStyle::Atx,
        hr_style: HrStyle::Dashes,
        br_style: BrStyle::TwoSpaces,
        link_style: LinkStyle::Inlined,
        link_reference_style: LinkReferenceStyle::Full,
        code_block_style: CodeBlockStyle::Fenced,
        code_block_fence: CodeBlockFence::Backticks,
        bullet_list_marker: BulletListMarker::Dash,
        ul_bullet_spacing: 1,
        ol_number_spacing: 1,
        preformatted_code: true,
        translation_mode: TranslationMode::Pure,
    };

    HtmlToMarkdown::builder()
        .options(options)
        .skip_tags(vec![
            "script", "style", "noscript", "form", "button", "svg", "nav", "input", "select",
            "template", "dialog",
        ])
        .add_handler(
            vec!["div", "section", "aside", "footer", "header"],
            |handlers: &dyn htmd::element_handler::Handlers, el: Element| {
                if let Some(c) = attr(&el, "class") {
                    if is_junk(&c) {
                        return Some(HandlerResult::from(""));
                    }
                }
                if let Some(id) = attr(&el, "id") {
                    if is_junk(&id) {
                        return Some(HandlerResult::from(""));
                    }
                }
                handlers.fallback(el)
            },
        )
        .add_handler(
            vec!["a"],
            |handlers: &dyn htmd::element_handler::Handlers, el: Element| {
                let href = attr(&el, "href").unwrap_or_default();
                let text = handlers.walk_children(el.node).content;
                let clean = clean_url(href.trim());
                if clean.is_empty() || clean.starts_with("javascript:") {
                    return Some(HandlerResult::from(text));
                }
                Some(HandlerResult::from(format!("[{}]({})", text.trim(), clean)))
            },
        )
        .build()
}

// ── Sanitizer: Zero Raw HTML Tags ────────────────────────────────────────

static HTML_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"</?[a-zA-Z][a-zA-Z0-9:-]*(\s+[^>]*)?/?>"#).expect("html tag regex")
});

static BLANKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n{3,}").expect("blanks regex"));
static TRAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)[ \t]+$").expect("trailing whitespace regex"));

pub fn strip_html_tags(s: &str) -> String {
    HTML_TAG_RE.replace_all(s, "").to_string()
}

/// Normalizes spacing and strips residual HTML tags outside fenced code blocks.
pub fn tidy_markdown(md: &str) -> String {
    let mut in_code_block = false;
    let mut sanitized_lines = Vec::new();

    for line in md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            sanitized_lines.push(line.to_string());
        } else if in_code_block {
            sanitized_lines.push(line.to_string());
        } else {
            let cleaned = strip_html_tags(line);
            let cleaned_trimmed = cleaned.trim();
            if cleaned_trimmed.starts_with('|') && cleaned_trimmed.ends_with('|') && cleaned_trimmed.len() > 1 {
                let is_separator = cleaned_trimmed
                    .chars()
                    .all(|c| c == '|' || c == '-' || c == ':' || c == ' ');
                if is_separator {
                    sanitized_lines.push(cleaned_trimmed.to_string());
                } else {
                    let parts: Vec<&str> = cleaned_trimmed
                        .trim_matches('|')
                        .split('|')
                        .map(str::trim)
                        .collect();
                    let normalized_row = format!("| {} |", parts.join(" | "));
                    sanitized_lines.push(normalized_row);
                }
            } else {
                sanitized_lines.push(cleaned);
            }
        }
    }

    let result = sanitized_lines.join("\n");
    let result = TRAIL.replace_all(&result, "");
    let result = BLANKS.replace_all(&result, "\n\n");
    format!("{}\n", result.trim())
}

// ── Universal Fetcher & Pipeline ─────────────────────────────────────────

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
        ),
    );
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    );
    headers.insert(
        ACCEPT_LANGUAGE,
        HeaderValue::from_static("en-US,en;q=0.9"),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .expect("build reqwest client")
});

/// Validates URL against SSRF and non-HTTP schemes.
pub fn validate_url(url_str: &str) -> Result<Url> {
    let trimmed = url_str.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("URL cannot be empty"));
    }
    let parsed = Url::parse(trimmed).map_err(|e| anyhow!("Invalid URL: {}", e))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        scheme => Err(anyhow!(
            "Unsupported or forbidden URL scheme: '{}'. Only HTTP and HTTPS are permitted.",
            scheme
        )),
    }
}

/// Executes the full universal web-to-markdown extraction.
pub async fn web_to_markdown(raw_url: &str) -> Result<WebMarkdownResult> {
    let validated_url = validate_url(raw_url)?;
    let cleaned_target_url = clean_url(validated_url.as_str());

    let res = HTTP_CLIENT
        .get(&cleaned_target_url)
        .send()
        .await
        .map_err(|e| anyhow!("Network error fetching {}: {}", cleaned_target_url, e))?;

    let status = res.status();
    if !status.is_success() {
        return Err(anyhow!(
            "HTTP error {}: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Error")
        ));
    }

    let raw_html = res
        .text()
        .await
        .map_err(|e| anyhow!("Failed to read response body: {}", e))?;

    let title = extract_title(&raw_html, &cleaned_target_url);
    let normalized_html = normalize_tables_for_htmd(&raw_html);
    let converted_md = CONVERTER.convert(&normalized_html).unwrap_or_default();
    let mut final_markdown = tidy_markdown(&converted_md);

    // If the converted markdown doesn't already contain the title as a heading, include it at the top
    if !final_markdown.contains(&title) && !title.is_empty() && title != "Web Page" {
        final_markdown = format!("# {}\n\n{}", title, final_markdown);
    }

    Ok(WebMarkdownResult {
        title,
        markdown: final_markdown,
        url: cleaned_target_url,
    })
}

/// JSON-RPC `tools/call` handler for `web_to_markdown`.
pub async fn call_web_to_markdown(arguments: Value) -> Result<Value, anyhow::Error> {
    let url_arg = arguments
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    if url_arg.is_empty() {
        return Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": "URL cannot be empty"
                }
            ],
            "isError": true
        }));
    }

    match web_to_markdown(url_arg).await {
        Ok(res) => {
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": res.markdown
                    }
                ],
                "title": res.title,
                "markdown": res.markdown,
                "url": res.url,
                "isError": false
            }))
        }
        Err(e) => {
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": e.to_string()
                    }
                ],
                "isError": true
            }))
        }
    }
}
