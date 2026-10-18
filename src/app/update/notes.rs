//! Release notes, drawn from their Markdown: headings, bullets, numbered
//! lists, paragraphs, **bold**, *italics*, `code` and links (shown as their
//! text). Raw HTML, such as an `<img>`, is left out. That's as much
//! Markdown as release notes written for people use.

use eframe::egui::{self, text::LayoutJob, Color32, FontFamily, FontId, TextFormat};

use super::super::theme;

/// A run of text in one style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// Link text, which is shown in the accent colour.
    pub link: bool,
}

/// A block of the notes.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// `#` to `######`, by level (1 is the biggest).
    Heading(u8, Vec<Span>),
    /// A paragraph: its lines joined.
    Paragraph(Vec<Span>),
    /// A list item, nested by `depth`, with its bullet or number.
    Item { depth: usize, marker: String, spans: Vec<Span> },
    /// `---`
    Rule,
}

/// Splits Markdown into blocks.
pub fn parse(markdown: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph = String::new();
    let flush = |paragraph: &mut String, blocks: &mut Vec<Block>| {
        if !paragraph.trim().is_empty() {
            blocks.push(Block::Paragraph(inline(paragraph.trim())));
        }
        paragraph.clear();
    };

    for line in markdown.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();

        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else if trimmed.starts_with('<') {
            // HTML: an image or a comment, which has no place in the app
            flush(&mut paragraph, &mut blocks);
        } else if let Some((level, text)) = heading(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(level, inline(text)));
        } else if trimmed.len() >= 3 && (trimmed.chars().all(|c| c == '-') || trimmed.chars().all(|c| c == '*')) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
        } else if let Some((marker, text)) = list_item(trimmed) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Block::Item { depth: indent / 2, marker, spans: inline(text) });
        } else if indent >= 2 && matches!(blocks.last(), Some(Block::Item { .. })) && paragraph.is_empty() {
            // A list item's text carried onto the next line
            if let Some(Block::Item { spans, .. }) = blocks.last_mut() {
                spans.push(Span { text: " ".into(), ..Default::default() });
                spans.extend(inline(trimmed));
            }
        } else {
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(trimmed);
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.chars().take_while(|&c| c == '#').count();
    let text = line[level..].strip_prefix(' ')?;
    (1..=6).contains(&level).then(|| (level as u8, text.trim().trim_end_matches('#').trim()))
}

fn list_item(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(bullet) {
            return Some(("•".into(), text));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(text) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((format!("{}.", &line[..digits]), text));
        }
    }
    None
}

/// Splits a line into styled spans.
pub fn inline(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut current = Span::default();
    let (mut bold, mut italic) = (false, false);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    let push = |spans: &mut Vec<Span>, current: &mut Span| {
        if !current.text.is_empty() {
            spans.push(std::mem::take(current));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\\' && next.is_some_and(|n| n.is_ascii_punctuation()) {
            current.text.push(next.unwrap());
            i += 2;
        } else if c == '`' {
            // Code runs to the next backtick, taken literally
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '`') {
                push(&mut spans, &mut current);
                spans.push(Span { text: chars[i + 1..i + 1 + end].iter().collect(), code: true, ..Default::default() });
                current = Span { bold, italic, ..Default::default() };
                i += end + 2;
            } else {
                current.text.push(c);
                i += 1;
            }
        } else if (c == '*' || c == '_') && next == Some(c) {
            push(&mut spans, &mut current);
            bold = !bold;
            current = Span { bold, italic, ..Default::default() };
            i += 2;
        } else if (c == '*' || c == '_') && emphasis_toggles(&chars, i, italic) {
            push(&mut spans, &mut current);
            italic = !italic;
            current = Span { bold, italic, ..Default::default() };
            i += 1;
        } else if c == '[' {
            // [text](url) shows its text
            let close_label = chars[i + 1..].iter().position(|&c| c == ']' || c == '[').map(|j| i + 1 + j);
            let link = close_label
                .filter(|&j| chars[j] == ']' && chars.get(j + 1) == Some(&'('))
                .and_then(|j| chars[j + 2..].iter().position(|&c| c == ')').map(|k| (j, j + 2 + k)));
            match link {
                Some((label_end, url_end)) => {
                    push(&mut spans, &mut current);
                    let label: String = chars[i + 1..label_end].iter().collect();
                    for mut span in inline(&label) {
                        span.link = true;
                        span.bold |= bold;
                        span.italic |= italic;
                        spans.push(span);
                    }
                    current = Span { bold, italic, ..Default::default() };
                    i = url_end + 1;
                }
                _ => {
                    current.text.push(c);
                    i += 1;
                }
            }
        } else {
            current.text.push(c);
            i += 1;
        }
    }
    push(&mut spans, &mut current);
    spans
}

/// Whether a lone `*` or `_` at `i` opens or closes emphasis, rather than
/// being a multiplication sign or part of a snake_case name.
fn emphasis_toggles(chars: &[char], i: usize, open: bool) -> bool {
    let before = i.checked_sub(1).map(|j| chars[j]);
    let after = chars.get(i + 1).copied();
    if open {
        before.is_some_and(|b| !b.is_whitespace())
    } else {
        let word_before = before.is_some_and(char::is_alphanumeric);
        after.is_some_and(|a| !a.is_whitespace()) && !(chars[i] == '_' && word_before)
    }
}

/// Draws parsed notes, wrapping to the width available.
pub fn show(ui: &mut egui::Ui, blocks: &[Block]) {
    let body = ui.style().text_styles[&egui::TextStyle::Body].size;
    for (n, block) in blocks.iter().enumerate() {
        match block {
            Block::Heading(level, spans) => {
                if n > 0 {
                    ui.add_space(6.0);
                }
                let size = match level {
                    1 => body + 5.0,
                    2 => body + 3.0,
                    _ => body + 1.0,
                };
                ui.label(job(ui, spans, size, theme::text::PRIMARY, true));
                ui.add_space(2.0);
            }
            Block::Paragraph(spans) => {
                ui.label(job(ui, spans, body, theme::text::SECONDARY, false));
                ui.add_space(4.0);
            }
            Block::Item { depth, marker, spans } => {
                ui.horizontal_top(|ui| {
                    ui.add_space(4.0 + 16.0 * *depth as f32);
                    let marker_width = if marker == "•" { 12.0 } else { 20.0 };
                    ui.add_sized(
                        [marker_width, body + 2.0],
                        egui::Label::new(egui::RichText::new(marker).color(theme::accent::PRIMARY)),
                    );
                    // A row doesn't wrap its labels unless told to, and an
                    // unwrapped bullet would hold the window open to its length
                    ui.add(egui::Label::new(job(ui, spans, body, theme::text::SECONDARY, false)).wrap());
                });
                ui.add_space(2.0);
            }
            Block::Rule => {
                ui.separator();
            }
        }
    }
}

fn job(ui: &egui::Ui, spans: &[Span], size: f32, color: Color32, heading: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let title = FontFamily::Name(theme::TITLE_FAMILY.into());
    for span in spans {
        let strong = heading || span.bold;
        let font_id = if span.code {
            FontId::monospace(size - 1.0)
        } else if strong {
            FontId::new(size, title.clone())
        } else {
            FontId::proportional(size)
        };
        let color = if span.link {
            theme::text::ACCENT
        } else if strong {
            theme::text::PRIMARY
        } else {
            color
        };
        job.append(
            &span.text,
            0.0,
            TextFormat {
                font_id,
                color,
                italics: span.italic,
                background: if span.code { theme::background::WIDGET } else { Color32::TRANSPARENT },
                ..Default::default()
            },
        );
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn blocks_are_headings_lists_and_paragraphs() {
        let md = "## Downloads\n\nIntro line one\ncontinues here.\n\n- **Windows**: `a.zip`\n  carried on\n- Linux\n  1. nested\n\n---\n<img src=\"x.png\">\n## v0.3.0 ##";
        let blocks = parse(md);
        assert_eq!(blocks.len(), 7, "{blocks:#?}");
        assert!(matches!(&blocks[0], Block::Heading(2, s) if text(s) == "Downloads"));
        assert!(matches!(&blocks[1], Block::Paragraph(s) if text(s) == "Intro line one continues here."));
        match &blocks[2] {
            Block::Item { depth: 0, marker, spans } => {
                assert_eq!(marker, "•");
                assert_eq!(text(spans), "Windows: a.zip carried on");
                assert!(spans[0].bold && spans[0].text == "Windows");
                assert!(spans.iter().any(|s| s.code && s.text == "a.zip"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&blocks[4], Block::Item { depth: 1, marker, .. } if marker == "1."));
        assert_eq!(blocks[5], Block::Rule);
        assert!(matches!(&blocks[6], Block::Heading(2, s) if text(s) == "v0.3.0"));
    }

    #[test]
    fn inline_styles_and_links() {
        let spans = inline("Or [try it in the browser](https://soba.chaps.dev/) first, *gently*.");
        assert_eq!(text(&spans), "Or try it in the browser first, gently.");
        assert!(spans.iter().any(|s| s.link && s.text == "try it in the browser"));
        assert!(spans.iter().any(|s| s.italic && s.text == "gently"));

        // Not emphasis: a product, a snake_case name, a lone star
        assert_eq!(text(&inline("2 * 3 and soba_v2 *")), "2 * 3 and soba_v2 *");
        assert!(inline("soba_v2").iter().all(|s| !s.italic));
        // Unclosed things stay as typed
        assert_eq!(text(&inline("a `b and [c")), "a `b and [c");
        assert_eq!(text(&inline(r"\*not\*")), "*not*");
    }

    #[test]
    fn the_real_notes_parse() {
        let releases = super::super::release::parse_releases(include_str!("testdata/releases.json")).unwrap();
        for release in &releases {
            let blocks = parse(&release.notes);
            let all: String = blocks
                .iter()
                .map(|b| match b {
                    Block::Heading(_, s) | Block::Paragraph(s) | Block::Item { spans: s, .. } => text(s),
                    Block::Rule => String::new(),
                })
                .collect();
            assert!(!all.contains("**"), "{}: {all}", release.version);
            assert!(!all.contains("<img"), "{}", release.version);
        }
    }

    #[test]
    fn the_notes_wrap_to_a_narrow_window() {
        let releases = super::super::release::parse_releases(include_str!("testdata/releases.json")).unwrap();
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        // The first frame loads the fonts
        let _ = ctx.run_ui(input(), |_| {});
        let column = 300.0;
        let mut widest = 0.0_f32;
        let _ = ctx.run_ui(input(), |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                for release in &releases {
                    // A column of its own: the panel's rect starts out full width
                    let used = ui.allocate_ui(egui::vec2(column, f32::INFINITY), |ui| show(ui, &parse(&release.notes)));
                    widest = widest.max(used.response.rect.width());
                }
            });
        });
        assert!(widest <= column + 1.0, "the notes reach {widest} px in a {column} px column");
    }
}
