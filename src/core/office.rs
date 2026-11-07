//! Reads the text of Word (.docx), PowerPoint (.pptx) and Excel (.xlsx) files. They are zip archives of XML, so a
//! small zip reader (flate2 does the inflating) and a tag scanner are all that is needed. Nothing is sent anywhere.
use std::io::Read;
use std::path::Path;

/// Most text taken from one file; the rest is cut with a note.
pub const MAX_OUT: usize = 400 * 1024;
/// Biggest single part inflated (a zip bomb can't use more memory than this).
const MAX_PART: u64 = 64 * 1024 * 1024;

pub fn is_office(name: &str) -> bool {
    matches!(name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref(), Some("docx" | "pptx" | "xlsx"))
}

struct Zip { data: Vec<u8>, ents: Vec<Ent> }
struct Ent { name: String, method: u16, off: usize, csize: usize }

fn u16at(d: &[u8], i: usize) -> Option<usize> { Some(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?) as usize) }
fn u32at(d: &[u8], i: usize) -> Option<usize> { Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?) as usize) }

impl Zip {
    fn open(data: Vec<u8>) -> Option<Zip> {
        let end = (0..data.len().checked_sub(21)?).rev().take(70_000).find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])?;
        let (n, mut p) = (u16at(&data, end + 10)?, u32at(&data, end + 16)?);
        let mut ents = Vec::new();
        for _ in 0..n {
            if data.get(p..p + 4)? != [0x50, 0x4b, 0x01, 0x02] { break; }
            let (nl, el, cl) = (u16at(&data, p + 28)?, u16at(&data, p + 30)?, u16at(&data, p + 32)?);
            let name = String::from_utf8_lossy(data.get(p + 46..p + 46 + nl)?).into_owned();
            ents.push(Ent { name, method: u16at(&data, p + 10)? as u16, off: u32at(&data, p + 42)?, csize: u32at(&data, p + 20)? });
            p += 46 + nl + el + cl;
        }
        Some(Zip { data, ents })
    }

    fn read(&self, name: &str) -> Option<String> {
        let e = self.ents.iter().find(|e| e.name == name)?;
        let d = &self.data;
        if d.get(e.off..e.off + 4)? != [0x50, 0x4b, 0x03, 0x04] { return None; }
        let start = e.off + 30 + u16at(d, e.off + 26)? + u16at(d, e.off + 28)?;
        let raw = d.get(start..start + e.csize)?;
        let mut out = Vec::new();
        match e.method {
            0 => out.extend_from_slice(raw),
            8 => { flate2::read::DeflateDecoder::new(raw).take(MAX_PART).read_to_end(&mut out).ok()?; }
            _ => return None,
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    /// Names like `ppt/slides/slide12.xml` under `dir`, in numeric order.
    fn numbered(&self, dir: &str, stem: &str) -> Vec<String> {
        let mut v: Vec<(u32, String)> = self.ents.iter().filter_map(|e| {
            let n = e.name.strip_prefix(dir)?.strip_prefix(stem)?.strip_suffix(".xml")?;
            Some((n.parse().ok()?, e.name.clone()))
        }).collect();
        v.sort();
        v.into_iter().map(|x| x.1).collect()
    }
}

fn unescape(s: &str, out: &mut String) {
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find(';').filter(|j| *j <= 8) else { out.push('&'); rest = after; continue };
        match &after[..j] {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            e => match e.strip_prefix('#').and_then(|n| match n.strip_prefix(['x', 'X']) { Some(h) => u32::from_str_radix(h, 16).ok(), None => n.parse().ok() }).and_then(char::from_u32) {
                Some(c) => out.push(c),
                None => { out.push('&'); rest = after; continue }
            },
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
}

/// The tags and text of an XML part, in order: `Tag(inner text of the <...>)` or `Text(..)`.
fn scan(xml: &str, mut f: impl FnMut(&str, Option<&str>)) {
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        if i > 0 { f("", Some(&rest[..i])); }
        let Some(j) = rest[i..].find('>') else { return };
        f(&rest[i + 1..i + j], None);
        rest = &rest[i + j + 1..];
    }
}

fn name_of(tag: &str) -> (&str, bool, bool) {
    let closing = tag.starts_with('/');
    let t = tag.trim_start_matches('/');
    let end = t.find([' ', '/', '\n', '\t', '\r']).unwrap_or(t.len());
    (&t[..end], closing, tag.ends_with('/'))
}

fn attr<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!(" {key}=\"");
    let i = tag.find(&pat)? + pat.len();
    Some(&tag[i..i + tag[i..].find('"')?])
}

/// Text runs (`t_tag`) grouped into one string per `sep` element, e.g. a paragraph.
fn segments(xml: &str, t_tag: &str, sep: &str) -> Vec<String> {
    let (mut out, mut cur, mut in_t) = (Vec::new(), String::new(), false);
    scan(xml, |tag, text| {
        if let Some(t) = text { if in_t { unescape(t, &mut cur); } return; }
        let (n, closing, selfc) = name_of(tag);
        if n == t_tag && !selfc { in_t = !closing; }
        else if closing && n == sep { out.push(std::mem::take(&mut cur)); }
        else if !closing && matches!(n, "w:tab") { cur.push('\t'); }
        else if !closing && matches!(n, "w:br" | "a:br") { cur.push('\n'); }
    });
    if !cur.is_empty() { out.push(cur); }
    out
}

fn column(cell: &str) -> usize {
    cell.bytes().take_while(u8::is_ascii_alphabetic).fold(0usize, |a, b| a * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize).saturating_sub(1)
}

fn sheet(xml: &str, shared: &[String]) -> String {
    let (mut out, mut row): (String, Vec<String>) = (String::new(), Vec::new());
    let (mut col, mut kind, mut val) = (0usize, String::new(), String::new());
    let (mut in_v, mut in_t) = (false, false);
    scan(xml, |tag, text| {
        if let Some(t) = text { if in_v || in_t { unescape(t, &mut val); } return; }
        let (n, closing, selfc) = name_of(tag);
        match (n, closing) {
            ("c", false) => {
                col = attr(tag, "r").map_or(row.len(), column);
                kind = attr(tag, "t").unwrap_or("").to_string();
                val.clear();
            }
            ("v", _) if !selfc => in_v = !closing,
            ("t", _) if !selfc => in_t = !closing,
            ("c", true) => {
                let s = if kind == "s" { val.trim().parse::<usize>().ok().and_then(|i| shared.get(i).cloned()).unwrap_or_default() } else { val.clone() };
                if !s.is_empty() {
                    if row.len() <= col { row.resize(col + 1, String::new()); }
                    row[col] = s.replace(['\t', '\n', '\r'], " ");
                }
            }
            ("row", true) => {
                if row.iter().any(|c| !c.is_empty()) { out.push_str(&row.join("\t")); out.push('\n'); }
                row.clear();
            }
            _ => {}
        }
    });
    out
}

fn cap(mut s: String) -> String {
    if s.len() > MAX_OUT {
        let mut n = MAX_OUT;
        while !s.is_char_boundary(n) { n -= 1; }
        s.truncate(n);
        s.push_str("\n…(cut: the file has more text than fits)");
    }
    s
}

/// The readable text of an Office file, or None when it isn't one this can read (or is damaged / password protected).
pub fn extract(path: &Path, name: &str) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > super::attach::MAX_FILE { return None; }
    let z = Zip::open(std::fs::read(path).ok()?)?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    let text = match ext.as_str() {
        "docx" => segments(&z.read("word/document.xml")?, "w:t", "w:p").join("\n"),
        "pptx" => {
            let notes = z.numbered("ppt/notesSlides/", "notesSlide");
            let mut o = String::new();
            for (i, s) in z.numbered("ppt/slides/", "slide").iter().enumerate() {
                o += &format!("--- Slide {} ---\n{}\n", i + 1, segments(&z.read(s)?, "a:t", "a:p").join("\n"));
                if let Some(n) = notes.iter().find(|n| n.ends_with(&format!("notesSlide{}.xml", s.rsplit("slide").next().unwrap_or("").trim_end_matches(".xml")))) {
                    let t = segments(&z.read(n).unwrap_or_default(), "a:t", "a:p").into_iter().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
                    if !t.trim().is_empty() { o += &format!("Notes: {t}\n"); }
                }
                if o.len() > MAX_OUT { break; }
            }
            o
        }
        "xlsx" => {
            let shared = z.read("xl/sharedStrings.xml").map(|x| segments(&x, "t", "si")).unwrap_or_default();
            let mut names = Vec::new();
            if let Some(wb) = z.read("xl/workbook.xml") {
                scan(&wb, |tag, t| if t.is_none() && name_of(tag) == ("sheet", false, true) || (t.is_none() && name_of(tag).0 == "sheet" && !name_of(tag).1) {
                    if let Some(n) = attr(tag, "name") { let mut s = String::new(); unescape(n, &mut s); names.push(s); }
                });
            }
            let mut o = String::new();
            for (i, s) in z.numbered("xl/worksheets/", "sheet").iter().enumerate() {
                o += &format!("--- Sheet: {} ---\n{}\n", names.get(i).map_or(format!("{}", i + 1), |n| n.clone()), sheet(&z.read(s)?, &shared));
                if o.len() > MAX_OUT { break; }
            }
            o
        }
        _ => return None,
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then(|| cap(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unescapes_entities() {
        let mut s = String::new();
        unescape("a &amp; b &lt;c&gt; &#65;&#x42; &bogus x", &mut s);
        assert_eq!(s, "a & b <c> AB &bogus x");
    }
    #[test]
    fn reads_paragraphs_and_cells() {
        let p = segments(r#"<w:p><w:r><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:t>world</w:t></w:r></w:p><w:p><w:t>Two</w:t></w:p>"#, "w:t", "w:p");
        assert_eq!(p, vec!["Hello world", "Two"]);
        let x = r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1"><v>5</v></c></row>"#;
        assert_eq!(sheet(x, &["Name".to_string()]), "Name\t\t5\n");
        assert_eq!(column("AA3"), 26);
    }
    #[test]
    fn reads_a_real_zip_container() {
        use std::io::Write;
        let body = br#"<w:document><w:p><w:r><w:t>Hi &amp; bye</w:t></w:r></w:p></w:document>"#;
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(body).unwrap();
        let comp = enc.finish().unwrap();
        let name = b"word/document.xml";
        let mut z = Vec::new();
        z.extend([0x50, 0x4b, 3, 4, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend((comp.len() as u32).to_le_bytes());
        z.extend((body.len() as u32).to_le_bytes());
        z.extend((name.len() as u16).to_le_bytes());
        z.extend([0, 0]);
        z.extend(name);
        z.extend(&comp);
        let cd = z.len() as u32;
        z.extend([0x50, 0x4b, 1, 2, 20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend((comp.len() as u32).to_le_bytes());
        z.extend((body.len() as u32).to_le_bytes());
        z.extend((name.len() as u16).to_le_bytes());
        z.extend([0u8; 12]);
        z.extend(0u32.to_le_bytes());
        z.extend(name);
        let cd_size = z.len() as u32 - cd;
        z.extend([0x50, 0x4b, 5, 6, 0, 0, 0, 0, 1, 0, 1, 0]);
        z.extend(cd_size.to_le_bytes());
        z.extend(cd.to_le_bytes());
        z.extend([0, 0]);
        let p = std::env::temp_dir().join("litepilot_office_test.docx");
        std::fs::write(&p, &z).unwrap();
        assert_eq!(extract(&p, "x.docx").as_deref(), Some("Hi & bye"));
        let _ = std::fs::remove_file(&p);
    }
    #[test]
    fn is_office_by_extension() {
        assert!(is_office("a.DOCX") && is_office("b.xlsx") && !is_office("c.doc") && !is_office("d"));
    }
}
