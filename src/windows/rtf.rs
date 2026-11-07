//! Markdown to RTF for RichEdit. Handles streaming partial Markdown without failing.
use crate::core::Msg;

pub struct Theme { pub bg: u32, pub text: u32, pub muted: u32, pub accent: u32, pub code: u32, pub bubble: u32, pub string: u32, pub comment: u32, pub keyword: u32, pub font: String, pub size: u32, pub spacing: u32 }

fn rgb(c: u32) -> String {
    format!("\\red{}\\green{}\\blue{};", c & 255, (c >> 8) & 255, (c >> 16) & 255)
}

/// RichEdit draws all text in a table cell white when the cell's shading is dark (HSL lightness about 50/255
/// or lower), which wipes out syntax colors. Lifts a shading color evenly until it is just past that point.
fn cell_shade(c: u32) -> u32 {
    const MIN_L: u32 = 56;
    let ch = [c & 255, (c >> 8) & 255, (c >> 16) & 255];
    let l = (ch.iter().max().unwrap() + ch.iter().min().unwrap()) / 2;
    if l >= MIN_L { return c; }
    let d = MIN_L - l;
    ch.iter().enumerate().fold(0, |acc, (i, v)| acc | ((v + d).min(255) << (8 * i)))
}

fn esc(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\t' => out.push_str("\\tab "),
            c if (c as u32) < 128 => out.push(c),
            c => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) { out.push_str(&format!("\\u{}?", *u as i16)); }
            }
        }
    }
}

fn inline(out: &mut String, s: &str) {
    let mut rest = s;
    let (mut bold, mut ital) = (false, false);
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("**") {
            bold = !bold;
            out.push_str(if bold { "\\b " } else { "\\b0 " });
            rest = r;
        } else if rest.starts_with('*') && (ital || !rest[1..].starts_with(' ')) {
            ital = !ital;
            out.push_str(if ital { "\\i " } else { "\\i0 " });
            rest = &rest[1..];
        } else if let Some((code, r)) = rest.strip_prefix('`').and_then(|r| r.split_once('`')) {
            out.push_str("{\\f1\\highlight4 ");
            esc(out, code);
            out.push('}');
            rest = r;
        } else if let Some((img, label, url, r)) = link(rest) {
            if img { image_or_link(out, label, url); } else { link_field(out, label, url); }
            rest = r;
        } else if rest.starts_with("![") {
            // A picture still arriving ("![label](https://..." with no closing bracket yet): show nothing.
            break;
        } else {
            let n = rest.char_indices().skip(1).find(|(_, c)| matches!(c, '*' | '`' | '[' | '!')).map_or(rest.len(), |x| x.0);
            esc(out, &rest[..n]);
            rest = &rest[n..];
        }
    }
    if bold { out.push_str("\\b0 "); }
    if ital { out.push_str("\\i0 "); }
}

fn link(s: &str) -> Option<(bool, &str, &str, &str)> {
    let (img, r) = match s.strip_prefix("![") {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('[')?),
    };
    let (label, r) = r.split_once("](")?;
    // The link ends at its matching bracket, so titles like "Jumbo (water tower)" stay whole.
    let mut depth = 0;
    let end = r.char_indices().find(|&(_, c)| match c {
        '(' => { depth += 1; false }
        ')' if depth == 0 => true,
        ')' => { depth -= 1; false }
        _ => false,
    })?.0;
    Some((img, label, &r[..end], &r[end + 1..]))
}

fn link_field(out: &mut String, label: &str, url: &str) {
    // Only web links become clickable; model output must not launch other URI schemes.
    if !(url.starts_with("http://") || url.starts_with("https://")) || url.contains(['"', '\\', '{', '}']) {
        return esc(out, label);
    }
    out.push_str("{\\field{\\*\\fldinst{HYPERLINK \"");
    esc(out, url);
    out.push_str("\"}}{\\fldrslt{\\cf3\\ul ");
    esc(out, if label.is_empty() { url } else { label });
    out.push_str("}}}");
}

/// A run of pictures gets one reserved line, a single row of tiles (see images.rs): embedded ones
/// draw at once, web ones (HTTPS only) download when they scroll into view. Pictures that can't be
/// shown fall back to links or labels.
fn gallery_block(out: &mut String, pics: &[(&str, &str)]) -> bool {
    let Some((id, h)) = super::images::register_gallery(pics) else { return false };
    out.push_str(&format!("\\pard\\ltrpar\\sl-{h}\\slmult0\\sb60\\sa100{{\\cf6\\fs2 "));
    if let Some(c) = char::from_u32(super::images::FIRST_CHAR + id) { esc(out, &c.to_string()); }
    out.push_str("}\\par\\pard ");
    true
}

fn image_or_link(out: &mut String, label: &str, url: &str) {
    let name = if label.is_empty() { "image" } else { label };
    if url.starts_with("http://") || url.starts_with("https://") {
        return link_field(out, name, url);
    }
    if let Some(t) = url.strip_prefix("wiki:") {
        let page = format!("https://en.wikipedia.org/wiki/{}", t.trim().replace(' ', "_"));
        return link_field(out, name, &page);
    }
    out.push_str("{\\cf2 ");
    esc(out, &format!("[Image: {name}]"));
    out.push('}');
}

const CELL_EDGE: &str = "\\clbrdrt\\brdrs\\brdrw5\\brdrcf5\\clbrdrl\\brdrs\\brdrw5\\brdrcf5\\clbrdrb\\brdrs\\brdrw5\\brdrcf5\\clbrdrr\\brdrs\\brdrw5\\brdrcf5";
// A small gap after a card. Braced so its tiny font can't leak into the text below.
const CARD_GAP: &str = "{\\pard\\sa60\\fs8\\par}\\pard ";

thread_local! {
    /// The chat's usable width in twips, set by the window on resize; cards and tables fit inside it.
    static WIDTH: std::cell::Cell<usize> = const { std::cell::Cell::new(9900) };
    /// False: tables wrap to fit. True: tables keep their natural width and the chat scrolls sideways.
    static TABLE_SCROLL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
thread_local! {
    /// Where attached files live, so user messages can show their pictures as thumbnails.
    static ATT_DIR: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}
pub fn set_attach_dir(d: std::path::PathBuf) { ATT_DIR.set(Some(d)); }
pub fn set_width(twips: usize) { WIDTH.set(twips.max(3000)); }
pub fn set_table_scroll(on: bool) { TABLE_SCROLL.set(on); }
#[allow(non_snake_case)]
fn CARD_W() -> usize { WIDTH.get() }

fn copy_link(out: &mut String, text: String) {
    let label = super::code::register(text);
    // The address is never opened; app.rs recognizes it and copies instead.
    out.push_str(&format!("{{\\field{{\\*\\fldinst{{HYPERLINK \"{label}\"}}}}{{\\fldrslt{{\\cf3\\fs18 "));
    esc(out, "Copy");
    out.push_str("}}}");
}

/// The bar across the top of a card: a label on the left, Copy on the right. Code cards shade it; tables
/// leave it plain, so the table below reads as one clean card.
fn card_bar(out: &mut String, label: &str, text: String, width: usize, shaded: bool) {
    // RichEdit draws a light guide line around a cell with no border set, so the plain bar gets borders in the
    // page's own color (color 6) instead.
    let cell = if shaded { format!(r"{CELL_EDGE}\clcbpat5") } else { edges(6, 6, 6, 6) };
    out.push_str(&format!(r"\trowd\trgaph120{cell}\cellx{}{cell}\cellx{width}", width * 2 / 3));
    out.push_str(r"\pard\intbl\ltrpar\sb40\sa40{\fs18\cf2 ");
    esc(out, label);
    out.push_str(r"}\cell\pard\intbl\qr\sb40\sa40 ");
    copy_link(out, text);
    out.push_str(r"\cell\row\pard ");
}

/// Cell borders (top, left, bottom, right), each a color index from the table of colors.
fn edges(t: u8, l: u8, b: u8, r: u8) -> String {
    format!(r"\clbrdrt\brdrs\brdrw5\brdrcf{t}\clbrdrl\brdrs\brdrw5\brdrcf{l}\clbrdrb\brdrs\brdrw5\brdrcf{b}\clbrdrr\brdrs\brdrw5\brdrcf{r}")
}

fn is_keyword(w: &str) -> bool {
    matches!(w, "if" | "else" | "elif" | "for" | "foreach" | "while" | "return" | "function" | "func" | "def" | "fn" | "let" | "const"
        | "var" | "class" | "struct" | "enum" | "impl" | "pub" | "use" | "import" | "from" | "export" | "try" | "catch" | "throw"
        | "switch" | "case" | "break" | "continue" | "true" | "false" | "null" | "None" | "True" | "False" | "in" | "not" | "and" | "or")
}

fn colored(out: &mut String, idx: u8, s: &str) {
    // Plain switches, not a nested group: RichEdit drops nested group colors inside table cells.
    out.push_str(&format!("\\cf{idx} "));
    esc(out, s);
    out.push_str("\\cf1 ");
}

/// Light, language-neutral highlighting: comments, strings, keywords, and variables.
fn code_line(out: &mut String, line: &str) {
    let b: Vec<(usize, char)> = line.char_indices().collect();
    let at = |i: usize| b.get(i).map_or(line.len(), |x| x.0);
    let mut i = 0;
    while i < b.len() {
        let c = b[i].1;
        let prev_space = i == 0 || b[i - 1].1.is_whitespace();
        if prev_space && (c == '#' || line[b[i].0..].starts_with("//")) {
            return colored(out, 8, &line[b[i].0..]);
        }
        if c == '"' || c == '\'' {
            let mut j = i + 1;
            while j < b.len() && b[j].1 != c { j += if b[j].1 == '\\' { 2 } else { 1 }; }
            let end = (j + 1).min(b.len());
            colored(out, 7, &line[b[i].0..at(end)]);
            i = end;
        } else if c == '$' || c.is_alphabetic() || c == '_' {
            let mut j = i + 1;
            while j < b.len() && (b[j].1.is_alphanumeric() || b[j].1 == '_') { j += 1; }
            let word = &line[b[i].0..at(j)];
            if c == '$' || is_keyword(word) { colored(out, 9, word); } else { esc(out, word); }
            i = j;
        } else {
            esc(out, &line[b[i].0..at(i + 1)]);
            i += 1;
        }
    }
}

fn code_card(out: &mut String, lang: &str, lines: &[&str]) {
    card_bar(out, if lang.is_empty() { "Code" } else { lang }, lines.join("\r\n"), CARD_W(), true);
    out.push_str(&format!("\\trowd\\trgaph120{CELL_EDGE}\\clcbpat4\\cellx{}\\pard\\intbl\\ltrpar\\sb60\\sa60{{\\f1\\fs19 ", CARD_W()));
    for (n, l) in lines.iter().enumerate() {
        if n > 0 { out.push_str("\\line "); }
        code_line(out, l);
    }
    out.push_str("}\\cell\\row\\pard ");
    out.push_str(CARD_GAP);
}

fn is_table_sep(l: &str) -> bool {
    let t = l.trim();
    t.contains('-') && t.contains('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn cells(l: &str) -> Vec<&str> {
    l.trim().trim_matches('|').split('|').map(str::trim).collect()
}

/// Right edges of each column in twips. Columns get room in proportion to their longest cell (so a
/// short label column stays narrow), with a floor so none is squeezed. Wrapping tables fit the chat;
/// scrolling tables keep the room their text needs.
fn column_edges(head: &[&str], rows: &[&str], n: usize) -> Vec<usize> {
    const CHAR_TW: usize = 105; // an average character at the body size, in twips
    const PAD_TW: usize = 380; // the cell's inner gap and border
    let mut need = vec![3usize; n];
    for (i, c) in head.iter().enumerate().take(n) { need[i] = need[i].max(c.chars().count() + 1); }
    for l in rows {
        for (i, c) in cells(l).iter().enumerate().take(n) { need[i] = need[i].max(c.chars().count()); }
    }
    let natural: Vec<usize> = need.iter().map(|c| (*c).min(60) * CHAR_TW + PAD_TW).collect();
    let total: usize = natural.iter().sum();
    let room = CARD_W();
    let widths: Vec<usize> = if TABLE_SCROLL.get() || total <= room {
        natural
    } else {
        // Share the room by need, but give every column at least a fair minimum.
        let floor = (room / n).min(1400);
        let spare = room.saturating_sub(floor * n);
        let extra: usize = natural.iter().map(|w| w.saturating_sub(floor)).sum::<usize>().max(1);
        natural.iter().map(|w| floor + w.saturating_sub(floor) * spare / extra).collect()
    };
    widths.iter().scan(0, |acc, w| { *acc += w; Some(*acc) }).collect()
}

/// One table row in the clean card style: a tinted, muted header, thin lines between rows, no vertical lines,
/// and roomy cells.
fn table_row(out: &mut String, cs: &[&str], edges_tw: &[usize], head: bool) {
    let n = edges_tw.len();
    out.push_str(r"\trowd\trgaph140");
    for i in 1..=n {
        // Header: tinted, its edges in its own tint. Body: a thin line above and below, page-colored sides.
        let look = if head { format!(r"{}\clcbpat4", edges(4, 4, 4, 4)) } else { edges(5, 6, 5, 6) };
        out.push_str(&format!(r"{look}\cellx{}", edges_tw[i - 1]));
    }
    for i in 0..n {
        out.push_str(r"\pard\intbl\sb70\sa70 ");
        if head { out.push_str(r"\b\cf2 "); }
        inline(out, cs.get(i).copied().unwrap_or(""));
        if head { out.push_str(r"\b0\cf1 "); }
        out.push_str(r"\cell ");
    }
    out.push_str(r"\row\pard ");
}

/// True when the first letter is from a right-to-left script (Arabic, Hebrew, Persian, Urdu and so on).
pub fn rtl(s: &str) -> bool {
    s.chars().find(|c| c.is_alphabetic()).is_some_and(|c| matches!(c as u32, 0x590..=0x8FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF))
}

/// Pictures on their own lines, outside code blocks, in reply order: (line index, label, url).
fn picture_lines<'a>(lines: &[&'a str]) -> Vec<(usize, &'a str, &'a str)> {
    let mut out = Vec::new();
    let mut fenced = false;
    for (i, &l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with("```") { fenced = !fenced; continue; }
        if fenced { continue; }
        if let Some((true, label, url, "")) = link(t.trim_end()) { out.push((i, label, url)); }
    }
    out
}

fn markdown(out: &mut String, md: &str) {
    let lines: Vec<&str> = md.lines().collect();
    // Every picture on its own line joins one gallery row, drawn where the first picture is. Headings and
    // text between the pictures don't split the row.
    let pictures = picture_lines(&lines);
    let mut row_done = false;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let t = l.trim_start();
        if row_done && pictures.iter().any(|p| p.0 == i) { i += 1; continue; }
        if t.starts_with("```") {
            let lang = t.trim_start_matches('`').trim();
            i += 1;
            let start = i;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") { i += 1; }
            code_card(out, lang, &lines[start..i]);
            i += 1;
            continue;
        }
        if t.starts_with('|') && lines.get(i + 1).is_some_and(|n| is_table_sep(n)) {
            let head = cells(t);
            let n = head.len();
            let first = i + 2;
            let mut end = first;
            while end < lines.len() && lines[end].trim_start().starts_with('|') { end += 1; }
            // Copies as tab-separated text, which pastes into spreadsheets as a real table.
            let tsv: Vec<String> = std::iter::once(t).chain(lines[first..end].iter().copied()).map(|l| cells(l).join("\t")).collect();
            let edges = column_edges(&head, &lines[first..end], n);
            card_bar(out, "Table", tsv.join("\r\n"), *edges.last().unwrap_or(&CARD_W()), false);
            table_row(out, &head, &edges, true);
            for l in &lines[first..end] { table_row(out, &cells(l), &edges, false); }
            out.push_str(CARD_GAP);
            i = end;
            continue;
        }
        // The row is drawn at the first picture. If no picture in it can be shown, each line falls back to a link.
        if pictures.first().is_some_and(|p| p.0 == i) {
            let pics: Vec<(&str, &str)> = pictures.iter().map(|p| (p.1, p.2)).collect();
            if gallery_block(out, &pics) {
                row_done = true;
                i += 1;
                continue;
            }
        }
        let dir = if rtl(t) { "\\rtlpar\\qr" } else { "\\ltrpar" };
        let hashes = t.bytes().take_while(|b| *b == b'#').count();
        if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
            let size = [32, 28, 25, 23, 22, 22][hashes - 1];
            out.push_str(&format!("\\pard{dir}\\sb120\\sa40{{\\b\\fs{size} "));
            inline(out, t[hashes..].trim());
            out.push_str("}\\par\\pard ");
        } else if let Some(item) = t.strip_prefix("- ").or(t.strip_prefix("* ")) {
            out.push_str(&format!("\\pard{dir}\\fi-220\\li300\\sa30 \\u8226?\\tab "));
            inline(out, item);
            out.push_str("\\par\\pard ");
        } else if t.is_empty() {
            // Paragraph spacing already separates blocks.
        } else {
            out.push_str(&format!("\\pard{dir}\\sa80 "));
            inline(out, t);
            out.push_str("\\par\\pard ");
        }
        i += 1;
    }
}

/// Side padding inside a user bubble, in twips.
const BUBBLE_PAD: usize = 150;

/// Width in twips of the longest line of `text` in `font` at `size` half-points. The font is made
/// 1 twip per unit, so the answer doesn't depend on the screen's DPI.
fn text_twips(font: &str, size: u32, text: &str) -> usize {
    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::*;
    let name: Vec<u16> = font.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let f = CreateFontW(-(size as i32 * 10), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, name.as_ptr());
        let old = SelectObject(dc, f as _);
        let mut best = 0i32;
        for l in text.lines() {
            let u: Vec<u16> = l.replace('\t', "    ").encode_utf16().collect();
            let mut s = SIZE { cx: 0, cy: 0 };
            if GetTextExtentPoint32W(dc, u.as_ptr(), u.len() as i32, &mut s) != 0 { best = best.max(s.cx); }
        }
        SelectObject(dc, old);
        DeleteObject(f as _);
        DeleteDC(dc);
        if best == 0 { text.lines().map(|l| l.chars().count()).max().unwrap_or(0) * size as usize * 5 } else { best as usize }
    }
}

/// One message as a complete RTF document, ready for EM_STREAMIN.
/// Blank lines held under a reply that hasn't started yet; the status steps are drawn in them (app.rs).
pub fn status_space(lines: usize, th: &Theme) -> String {
    let mut o = format!("{{\\rtf1\\ansi\\deff0{{\\fonttbl{{\\f0 {};}}}}\\f0\\fs{} \\pard\\sb40\\sa40 ", th.font, th.size);
    for _ in 0..lines.max(1) { o.push_str("\\par "); }
    o.push('}');
    o
}

pub fn message(m: &Msg, th: &Theme) -> String {
    let mut o = String::with_capacity(m.text.len() + 512);
    o.push_str(&format!("{{\\rtf1\\ansi\\deff0\\uc1{{\\fonttbl{{\\f0 {};}}{{\\f1 Consolas;}}}}{{\\colortbl;", th.font));
    // Colors 4 (code) and 5 (bubble) shade table cells, so both must stay light enough for RichEdit.
    for c in [th.text, th.muted, th.accent, cell_shade(th.code), cell_shade(th.bubble), th.bg, th.string, th.comment, th.keyword] { o.push_str(&rgb(c)); }
    o.push_str(&format!("}}\\f0\\fs{}\\cf1 ", th.size));
    if m.role == "user" {
        // The bubble: one right-aligned shaded cell sized to the text. Zero-width marks at its first and last
        // character let the painter find it and round its corners (bubbles.rs). Text is shown as typed.
        // The widest line is measured in the real font (in twips) so the bubble hugs the words.
        // Attached pictures show as a row of small thumbnails above the bubble; their [📎 name] marks are dropped.
        let mut shown = m.text.clone();
        let pics: Vec<(String, std::path::PathBuf)> = ATT_DIR.with_borrow(|d| match d {
            Some(d) => crate::core::attach::names(&m.text).into_iter()
                .filter(|n| matches!(crate::core::attach::kind(n), crate::core::attach::Kind::Image(_)) && d.join(n).is_file())
                .map(|n| { let p = d.join(&n); (n, p) }).collect(),
            None => Vec::new(),
        });
        if let Some((id, h)) = (!pics.is_empty()).then(|| super::images::register_files(&pics, &m.text)).flatten() {
            o.push_str(&format!("\\pard\\ltrpar\\sl-{h}\\slmult0\\sb120\\sa40{{\\cf6\\fs2 "));
            if let Some(c) = char::from_u32(super::images::FIRST_CHAR + id) { esc(&mut o, &c.to_string()); }
            o.push_str("}\\par\\pard ");
            for (n, _) in &pics { shown = shown.replace(&crate::core::attach::token(n), ""); }
            shown = shown.trim().to_string();
        }
        if shown.is_empty() { o.push('}'); return o; }
        let m = &Msg::new("user", &shown);
        let dir = if rtl(&m.text) { "\\rtlpar\\qr" } else { "\\ltrpar" };
        let wide = (text_twips(&th.font, th.size, &m.text) + 2 * BUBBLE_PAD + 90).clamp(700, CARD_W() * 4 / 5);
        // A border in the bubble's own color; without one RichEdit draws a light guide line around the cell.
        o.push_str(&format!("{{\\pard\\sa40\\fs8\\par}}\\pard\\trowd\\trqr\\trgaph{BUBBLE_PAD}{CELL_EDGE}\\clcbpat5\\cellx{wide}\\pard\\intbl{dir}\\sb110\\sa110 \\u8203?"));
        for (i, l) in m.text.lines().enumerate() {
            if i > 0 { o.push_str("\\line "); }
            esc(&mut o, l);
        }
        o.push_str("\\u8203?\\cell\\row{\\pard\\sa40\\fs8\\par}\\pard ");
    } else if m.role == "note" {
        note(&mut o, &m.text);
    } else if m.role == "error" {
        o.push_str("\\pard\\sb80\\sa30{\\b\\fs18\\cf2 Error}\\par\\pard ");
        for l in m.text.lines() {
            o.push_str("\\pard\\sa60\\cf2 ");
            esc(&mut o, l);
            o.push_str("\\par\\pard ");
        }
    } else {
        markdown(&mut o, &m.text);
        // A small line after each reply. The speed under the newest one is drawn in it (app.rs), never as text.
        if m.role == "assistant" { o.push_str(r"\pard\sa0\sb0\fs20 \u8204?\par\pard "); }
    }
    o.push('}');
    if th.spacing != 100 { o = spaced(&o, th.spacing); }
    o
}

/// A note such as "Now using Claude · Opus": small, muted, and centered.
fn note(o: &mut String, text: &str) {
    o.push_str("\\pard\\qc\\sb120\\sa120{\\fs17\\cf2 ");
    esc(o, text);
    o.push_str("}\\par\\pard ");
}

/// Scales paragraph spacing (\sb and \sa) by a percentage. Escaped text can't match: a literal
/// backslash is written as two, so pairs are copied through untouched.
fn spaced(rtf: &str, pct: u32) -> String {
    let b = rtf.as_bytes();
    let mut out = String::with_capacity(rtf.len() + 64);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && b.get(i + 1) == Some(&b'\\') {
            out.push_str("\\\\");
            i += 2;
            continue;
        }
        if b[i] == b'\\' && b.get(i + 1) == Some(&b's') && matches!(b.get(i + 2), Some(b'a' | b'b')) && b.get(i + 3).is_some_and(u8::is_ascii_digit) {
            let mut j = i + 3;
            while j < b.len() && b[j].is_ascii_digit() { j += 1; }
            let n: u32 = rtf[i + 3..j].parse().unwrap_or(0);
            out.push_str(&rtf[i..i + 3]);
            out.push_str(&(n * pct / 100).to_string());
            i = j;
            continue;
        }
        let ch = rtf[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn th() -> Theme { Theme { bg: 0, text: 1, muted: 2, accent: 3, code: 4, bubble: 5, string: 6, comment: 7, keyword: 8, font: "Segoe UI".into(), size: 21, spacing: 100 } }
    fn r(s: &str) -> String { message(&Msg::new("assistant", s), &th()) }
    #[test]
    fn spacing_scales_only_control_words() {
        assert_eq!(spaced("\\pard\\sa80\\sb40 a\\\\sa80 b", 150), "\\pard\\sa120\\sb60 a\\\\sa80 b");
    }
    #[test]
    fn rtl_scripts_start_from_the_right() {
        assert!(rtl("\u{645}\u{631}\u{62D}\u{628}\u{627}") && rtl("  \u{5E9}\u{5DC}\u{5D5}\u{5DD}") && !rtl("hello") && !rtl("") && !rtl("123"));
        assert!(message(&Msg::new("user", "\u{645}\u{631}\u{62D}\u{628}\u{627}"), &th()).contains(r"\rtlpar"));
        assert!(message(&Msg::new("user", "hello"), &th()).contains(r"\ltrpar"));
    }
    #[test]
    fn user_bubble_is_one_marked_cell() {
        let o = message(&Msg::new("user", "one\ntwo"), &th());
        assert_eq!(o.matches("\\cell\\row").count(), 1);
        assert_eq!(o.matches("\\u8203?").count(), 2);
        assert!(o.contains("one\\line two"));
    }
    #[test]
    fn escapes_rtf_control_chars_and_unicode() {
        let o = r("a{b}\\c é 😀");
        assert!(o.contains("a\\{b\\}\\\\c \\u233? \\u-10179?\\u-8704?"));
    }
    #[test]
    fn table_and_code() {
        let o = r("| A | B |\n|---|---|\n| 1 | 2 |\n\n```\nx{y}\n```");
        assert_eq!(o.matches("\\row").count(), 5); // table bar and 2 rows, code bar and body
        assert!(o.contains("x\\{y\\}"));
        assert!(o.matches("HYPERLINK").count() == 2);
    }
    #[test]
    fn unsafe_links_are_not_clickable() {
        assert!(r("[x](https://a.b)").contains("HYPERLINK"));
        assert!(!r("[x](file:///c:/a.exe)").contains("HYPERLINK"));
    }
    #[test]
    fn images_reserve_space() {
        let d = r("![p](data:image/png;base64,iVBORw0KGgo=)");
        assert!(d.contains("[Image: p]") && !d.contains("iVBOR"));
        let ok = r("![p](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB)");
        assert!(ok.contains("\\sl-945\\slmult0") && !ok.contains("iVBOR")); // one row of 63px tiles
        let web = r("![a](https://a.b/x.png)\n\n![b](wiki:Colchester)\n![c](https://a.b/y.png)");
        assert!(web.matches("\\slmult0").count() == 1 && !web.contains("HYPERLINK")); // a run of pictures is one row
        assert!(r("![p](http://a.b/x.png)").contains("HYPERLINK")); // plain HTTP stays a link
    }
    #[test]
    fn pictures_share_one_row_across_headings() {
        let o = r("Here:\n**A**\n![a](https://a.b/x.png)\n\n**B**\n![b](https://a.b/y.png)\n\nText");
        assert_eq!(o.matches("\\slmult0").count(), 1); // both pictures in one row, drawn at the first
        assert!(!o.contains("HYPERLINK"));
        let code = r("```\n![a](https://a.b/x.png)\n```");
        assert!(!code.contains("\\slmult0")); // a picture inside a code block is just text
    }
    #[test]
    fn partial_markdown_keeps_braces_balanced() {
        for s in ["**bo", "`co", "```\ncode", "[a](", "| a |\n|---", "![x](data:image/png;base64,iV"] {
            let o = r(s);
            let d = o.replace("\\{", "").replace("\\}", "");
            assert_eq!(d.matches('{').count(), d.matches('}').count(), "{s}");
        }
    }
    #[test]
    fn code_copies_source_and_table_copies_tsv() {
        crate::windows::code::clear();
        r("| A | B |\n|---|---|\n| 1 | 2 |\n\n```sh\necho {x}\n```");
        assert_eq!(crate::windows::code::lookup("https://litepilot.invalid/copy/0").as_deref(), Some("A\tB\r\n1\t2"));
        assert_eq!(crate::windows::code::lookup("https://litepilot.invalid/copy/1").as_deref(), Some("echo {x}"));
        crate::windows::code::clear();
    }
    #[test]
    fn highlighting_keeps_all_text() {
        let mut o = String::new();
        code_line(&mut o, "if $a == \"b c\" # note");
        assert!(o.contains("\\cf9 if\\cf1 ") && o.contains("\\cf9 $a\\cf1 ") && o.contains("\\cf7 \"b c\"\\cf1 ") && o.contains("\\cf8 # note\\cf1 "));
    }
    #[test]
    fn dark_cell_shading_is_lifted() {
        assert_eq!(cell_shade(0xECE8E4), 0xECE8E4); // light themes are untouched
        assert_eq!(cell_shade(0x2D251D), 0x403830); // dark code: lightness 37 -> 56
        assert_eq!(cell_shade(0x000000), 0x383838);
        assert!(r("x").contains(&rgb(cell_shade(4))));
    }
    #[test]
    fn arabic_is_right_to_left() {
        assert!(r("مرحبا").contains("\\rtlpar"));
    }
}
