//! File → text for GraphRAG ingestion (2026-09-29).  By extension:
//! - pdf: text layer via pdf-extract; a PDF with (almost) no text layer is a
//!   scan — `needs_ocr` is set and the engine has Claude read its pages;
//! - pptx: one `# Slide N: <title>` chapter per slide (text boxes in order),
//!   speaker notes under `Notes:`;
//! - docx: `word/document.xml` paragraphs, headings (`Heading1…`, `Title`)
//!   written as markdown `#` lines so chunking sees the chapters;
//! - html/htm: tags dropped, h1–h3 kept as `#` lines;
//! - everything else that is valid UTF-8 (md, txt, csv, json, yaml, code…)
//!   as-is; markdown keeps its headings.

use std::io::Read;

#[derive(Debug, Clone, PartialEq)]
pub struct Extracted {
    pub text: String,
    /// pdf | docx | pptx | html | markdown | text
    pub format: &'static str,
    /// pages (pdf) or slides (pptx); 0 when the format has none
    pub pages: usize,
    /// a scanned PDF: too little text per page — OCR it (the engine's job)
    pub needs_ocr: bool,
}

/// Under this many non-blank characters per page a PDF counts as scanned.
pub const OCR_CHARS_PER_PAGE: usize = 40;

pub fn ext_of(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

pub fn extract(filename: &str, bytes: &[u8]) -> Result<Extracted, String> {
    let ext = ext_of(filename);
    let plain = |text: String, format: &'static str| Extracted { text, format, pages: 0, needs_ocr: false };
    let out = match ext.as_str() {
        "pdf" => {
            let pages = pdf_pages(bytes);
            let text = pdf(bytes).unwrap_or_default();
            let chars = text.chars().filter(|c| !c.is_whitespace()).count();
            let needs_ocr = chars < OCR_CHARS_PER_PAGE * pages.max(1);
            Extracted { text: if needs_ocr { String::new() } else { text }, format: "pdf", pages, needs_ocr }
        }
        "docx" => plain(docx(bytes)?, "docx"),
        "pptx" => {
            let (text, slides) = pptx(bytes)?;
            Extracted { text, format: "pptx", pages: slides, needs_ocr: false }
        }
        "doc" | "ppt" | "xls" => return Err(format!(".{ext} (the old binary Office format) is not supported; save it as .docx/.pptx/.pdf")),
        "html" | "htm" => plain(html(&utf8(bytes)?), "html"),
        "md" | "markdown" => plain(utf8(bytes)?, "markdown"),
        _ => {
            let t = utf8(bytes).map_err(|_| format!("{filename}: not a supported document type (pdf, docx, pptx, html, md, txt or other UTF-8 text)"))?;
            plain(t, "text")
        }
    };
    if out.needs_ocr {
        return Ok(out);
    }
    let text = normalise(&out.text);
    if text.trim().is_empty() {
        return Err(format!("{filename}: no text could be extracted"));
    }
    Ok(Extracted { text, ..out })
}

/// Normalise OCR'd or otherwise produced text the same way extraction does.
pub fn tidy(text: &str) -> String {
    normalise(text)
}

fn pdf_pages(bytes: &[u8]) -> usize {
    let owned = bytes.to_vec();
    std::panic::catch_unwind(move || lopdf::Document::load_mem(&owned).map(|d| d.get_pages().len()).unwrap_or(0)).unwrap_or(0)
}

/// Slides in order: `ppt/slides/slideN.xml` text paragraphs, then the slide's
/// notes (`ppt/notesSlides/notesSlideN.xml`) when it has any.
fn pptx(bytes: &[u8]) -> Result<(String, usize), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("pptx: {e}"))?;
    let mut slides: Vec<(usize, String)> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .filter_map(|n| {
            let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse::<usize>().ok()?;
            Some((num, n))
        })
        .collect();
    slides.sort();
    let mut read = |name: &str| -> Option<String> {
        let mut s = String::new();
        zip.by_name(name).ok()?.read_to_string(&mut s).ok()?;
        Some(s)
    };
    let mut out = String::new();
    for (num, name) in &slides {
        let paras = drawing_paragraphs(&read(name).unwrap_or_default());
        let title = paras.first().cloned().unwrap_or_default();
        out.push_str(&format!("# Slide {num}{}\n\n", if title.is_empty() { String::new() } else { format!(": {title}") }));
        for p in paras.iter().skip(1) {
            out.push_str(p);
            out.push_str("\n\n");
        }
        let notes = drawing_paragraphs(&read(&format!("ppt/notesSlides/notesSlide{num}.xml")).unwrap_or_default());
        // a notes page repeats the slide number as its own text box: keep real notes only
        let notes: Vec<&String> = notes.iter().filter(|n| n.trim() != num.to_string()).collect();
        if !notes.is_empty() {
            out.push_str("Notes:\n\n");
            for n in notes {
                out.push_str(n);
                out.push_str("\n\n");
            }
        }
    }
    Ok((out, slides.len()))
}

/// DrawingML text: each `<a:p>` paragraph's `<a:t>` runs joined.
pub fn drawing_paragraphs(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    for para in xml.split("<a:p>").flat_map(|p| p.split("<a:p ")).skip(1) {
        let body = para.split("</a:p>").next().unwrap_or("");
        let mut text = String::new();
        let mut rest = body;
        while let Some(i) = rest.find("<a:t") {
            let after_tag = match rest[i..].find('>') {
                Some(j) => &rest[i + j + 1..],
                None => break,
            };
            if rest[i..].starts_with("<a:t/>") || rest[i..].starts_with("<a:tab") || rest[i..].starts_with("<a:tbl") {
                rest = after_tag;
                continue;
            }
            let end = after_tag.find("</a:t>").unwrap_or(after_tag.len());
            text.push_str(&unescape(&after_tag[..end]));
            rest = &after_tag[end..];
        }
        let t = text.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    out
}

fn utf8(bytes: &[u8]) -> Result<String, String> {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(b.to_vec()).map_err(|e| format!("not UTF-8 text: {e}"))
}

/// CRLF → LF, trailing spaces off, runs of 3+ blank lines squeezed.
fn normalise(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blanks = 0;
    for line in s.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let l = line.trim_end();
        if l.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(l);
        out.push('\n');
    }
    out.trim().to_string()
}

fn pdf(bytes: &[u8]) -> Result<String, String> {
    // pdf-extract panics on some malformed files; a bad upload must not take the server down
    let owned = bytes.to_vec();
    std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem(&owned))
        .map_err(|_| "the PDF could not be read (parser failure)".to_string())?
        .map_err(|e| format!("pdf: {e}"))
}

fn docx(bytes: &[u8]) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("docx: {e}"))?;
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .map_err(|e| format!("docx: word/document.xml: {e}"))?
        .read_to_string(&mut xml)
        .map_err(|e| format!("docx: {e}"))?;
    Ok(docx_xml_to_text(&xml))
}

/// Walk `<w:p>` paragraphs: text runs `<w:t>`, tabs, breaks; a paragraph style
/// `Heading N` / `Title` becomes a markdown heading.
pub fn docx_xml_to_text(xml: &str) -> String {
    let mut out = String::new();
    for para in xml.split("<w:p ").flat_map(|p| p.split("<w:p>")).skip(1) {
        let body = para.split("</w:p>").next().unwrap_or("");
        let mut level = 0usize;
        if let Some(i) = body.find("<w:pStyle w:val=\"") {
            let v = &body[i + 17..];
            let style = v.split('"').next().unwrap_or("").to_ascii_lowercase();
            if style == "title" {
                level = 1;
            } else if let Some(n) = style.strip_prefix("heading").and_then(|n| n.trim().parse::<usize>().ok()) {
                level = (n + 1).min(6);
            }
        }
        let mut text = String::new();
        let mut rest = body;
        while let Some(i) = rest.find('<') {
            let tag_end = rest[i..].find('>').map(|j| i + j + 1).unwrap_or(rest.len());
            let tag = &rest[i..tag_end];
            if tag.starts_with("<w:t>") || tag.starts_with("<w:t ") {
                let after = &rest[tag_end..];
                let end = after.find("</w:t>").unwrap_or(after.len());
                text.push_str(&unescape(&after[..end]));
                rest = &after[end..];
                continue;
            }
            if tag.starts_with("<w:tab") {
                text.push('\t');
            } else if tag.starts_with("<w:br") || tag.starts_with("<w:cr") {
                text.push('\n');
            }
            rest = &rest[tag_end..];
        }
        let t = text.trim();
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        if level > 0 {
            out.push_str(&"#".repeat(level));
            out.push(' ');
        }
        out.push_str(t);
        out.push_str("\n\n");
    }
    out
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Crude but dependable: drop script/style, turn h1–h3 into `#` lines and
/// block ends into newlines, strip the remaining tags.
pub fn html(src: &str) -> String {
    let lower = src.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    let b = src.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            let end = lower[i..].find('>').map(|j| i + j + 1).unwrap_or(b.len());
            let tag = &lower[i..end];
            if let Some(skip) = ["<script", "<style"].iter().find(|s| tag.starts_with(**s)) {
                let close = format!("</{}", &skip[1..]);
                let j = lower[end..].find(&close).map(|j| end + j).unwrap_or(b.len());
                i = lower[j..].find('>').map(|k| j + k + 1).unwrap_or(b.len());
                continue;
            }
            match tag.get(..3) {
                Some("<h1") => out.push_str("\n# "),
                Some("<h2") => out.push_str("\n## "),
                Some("<h3") => out.push_str("\n### "),
                _ => {}
            }
            if ["</p", "</h", "<br", "</li", "</div", "</tr", "<li"].iter().any(|t| tag.starts_with(t)) {
                out.push('\n');
            }
            i = end;
        } else {
            let next = src[i..].find('<').map(|j| i + j).unwrap_or(b.len());
            out.push_str(&unescape(&src[i..next]).replace("&nbsp;", " "));
            i = next;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docx_headings_and_runs() {
        let xml = r#"<w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Intro</w:t></w:r></w:p><w:p w:rsidR="1"><w:r><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:t>&amp; welcome</w:t></w:r></w:p></w:body>"#;
        assert_eq!(normalise(&docx_xml_to_text(xml)), "## Intro\n\nHello & welcome");
    }

    #[test]
    fn pptx_slides_become_chapters_with_notes() {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            let slide = |t: &str, b: &str| format!("<p:sld><a:p><a:r><a:t>{t}</a:t></a:r></a:p><a:p><a:r><a:t>{b}</a:t></a:r><a:r><a:t> &amp; more</a:t></a:r></a:p></p:sld>");
            z.start_file("ppt/slides/slide2.xml", o).unwrap();
            z.write_all(slide("Plan", "Ship it").as_bytes()).unwrap();
            z.start_file("ppt/slides/slide1.xml", o).unwrap();
            z.write_all(slide("Goals", "Grow").as_bytes()).unwrap();
            z.start_file("ppt/notesSlides/notesSlide1.xml", o).unwrap();
            z.write_all(b"<p:notes><a:p><a:r><a:t>Say this slowly</a:t></a:r></a:p><a:p><a:r><a:t>1</a:t></a:r></a:p></p:notes>").unwrap();
            z.finish().unwrap();
        }
        let e = extract("deck.pptx", buf.get_ref()).unwrap();
        assert_eq!((e.format, e.pages), ("pptx", 2));
        assert_eq!(e.text, "# Slide 1: Goals\n\nGrow & more\n\nNotes:\n\nSay this slowly\n\n# Slide 2: Plan\n\nShip it & more");
        let (ch, _) = crate::chunk::chunk(&e.text);
        assert_eq!(ch.len(), 2);
    }

    #[test]
    fn html_keeps_headings_drops_scripts() {
        let t = normalise(&html("<html><script>var x=1;</script><h1>Title</h1><p>a &amp; b</p><p>c</p></html>"));
        assert_eq!(t, "# Title\na & b\nc");
    }

    #[test]
    fn refuses_binary_and_empty() {
        assert!(extract("x.bin", &[0xff, 0xfe, 0x00, 0x80]).is_err());
        assert!(extract("x.ppt", b"abc").unwrap_err().contains("pptx"));
        assert!(extract("x.txt", b"   \n ").is_err());
        assert!(extract("x.doc", b"abc").unwrap_err().contains("docx"));
        assert_eq!(extract("x.md", b"# A\r\n\r\n\r\n\r\nb").unwrap().text, "# A\n\nb");
    }
}
