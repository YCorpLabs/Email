//! Self-contained demo files: MIME messages, PNG pictures, PDFs and calendar
//! invitations, generated in code so the demo never downloads or bundles
//! third-party material.

/// One MIME part of a demo message.
#[derive(Debug, Clone)]
pub(crate) struct Part {
    pub filename: String,
    pub mime_type: String,
    /// Set for inline images referenced as `cid:` from the HTML body.
    pub content_id: Option<String>,
    /// Shown in the reader's attachment list (an inline logo is not).
    pub listed: bool,
    pub bytes: Vec<u8>,
}

impl Part {
    pub fn file(filename: &str, mime_type: &str, bytes: Vec<u8>) -> Self {
        Self {
            filename: filename.into(),
            mime_type: mime_type.into(),
            content_id: None,
            listed: true,
            bytes,
        }
    }
    pub fn inline(filename: &str, content_id: &str, bytes: Vec<u8>) -> Self {
        Self {
            filename: filename.into(),
            mime_type: "image/png".into(),
            content_id: Some(content_id.into()),
            listed: false,
            bytes,
        }
    }
    /// A photo shown in the message and also offered as an attachment.
    pub fn photo(filename: &str, content_id: &str, bytes: Vec<u8>) -> Self {
        Self {
            listed: true,
            ..Self::inline(filename, content_id, bytes)
        }
    }
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4 + bytes.len() / 57 * 2);
    for (index, chunk) in bytes.chunks(3).enumerate() {
        if index > 0 && index % 19 == 0 {
            out.push_str("\r\n");
        }
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A complete MIME message holding the bodies and every part, in the shape a
/// provider returns: multipart/mixed around multipart/related around
/// multipart/alternative. Parsed by the same attachment code as IMAP mail.
pub(crate) fn mime_message(plain: &str, html: Option<&str>, parts: &[Part]) -> Vec<u8> {
    let mut out = String::from("MIME-Version: 1.0\r\n");
    out.push_str("Content-Type: multipart/mixed; boundary=\"demo-mixed\"\r\n\r\n");
    out.push_str(
        "--demo-mixed\r\nContent-Type: multipart/related; boundary=\"demo-related\"\r\n\r\n",
    );
    out.push_str(
        "--demo-related\r\nContent-Type: multipart/alternative; boundary=\"demo-alt\"\r\n\r\n",
    );
    out.push_str("--demo-alt\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n");
    out.push_str(&base64(plain.as_bytes()));
    out.push_str("\r\n");
    if let Some(html) = html {
        out.push_str("--demo-alt\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n");
        out.push_str(&base64(html.as_bytes()));
        out.push_str("\r\n");
    }
    out.push_str("--demo-alt--\r\n");
    for part in parts.iter().filter(|p| p.content_id.is_some()) {
        out.push_str(&format!(
            "--demo-related\r\nContent-Type: {}; name=\"{}\"\r\nContent-Disposition: inline; filename=\"{}\"\r\nContent-ID: <{}>\r\nContent-Transfer-Encoding: base64\r\n\r\n",
            part.mime_type,
            part.filename,
            part.filename,
            part.content_id.as_deref().unwrap_or_default()
        ));
        out.push_str(&base64(&part.bytes));
        out.push_str("\r\n");
    }
    out.push_str("--demo-related--\r\n");
    for part in parts.iter().filter(|p| p.content_id.is_none()) {
        out.push_str(&format!(
            "--demo-mixed\r\nContent-Type: {}; name=\"{}\"\r\nContent-Disposition: attachment; filename=\"{}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n",
            part.mime_type, part.filename, part.filename
        ));
        out.push_str(&base64(&part.bytes));
        out.push_str("\r\n");
    }
    out.push_str("--demo-mixed--\r\n");
    out.into_bytes()
}

// ---------------------------------------------------------------- PNG ------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

struct Bits {
    out: Vec<u8>,
    value: u64,
    count: u32,
}
impl Bits {
    fn put(&mut self, value: u32, bits: u32) {
        self.value |= u64::from(value) << self.count;
        self.count += bits;
        while self.count >= 8 {
            self.out.push(self.value as u8);
            self.value >>= 8;
            self.count -= 8;
        }
    }
    /// Huffman codes are sent most significant bit first.
    fn code(&mut self, code: u32, bits: u32) {
        let mut reversed = 0;
        for i in 0..bits {
            reversed |= ((code >> i) & 1) << (bits - 1 - i);
        }
        self.put(reversed, bits);
    }
    fn literal(&mut self, value: u32) {
        match value {
            0..=143 => self.code(0x30 + value, 8),
            144..=255 => self.code(0x190 + value - 144, 9),
            256..=279 => self.code(value - 256, 7),
            _ => self.code(0xc0 + value - 280, 8),
        }
    }
}

/// zlib stream with fixed Huffman codes and distance-1 runs. Flat-colour
/// illustrations filtered with PNG's Up filter compress to a few kilobytes.
fn zlib(data: &[u8]) -> Vec<u8> {
    const BASE: [u32; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const EXTRA: [u32; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    let mut bits = Bits {
        out: vec![0x78, 0x01],
        value: 0,
        count: 0,
    };
    bits.put(1, 1); // final block
    bits.put(1, 2); // fixed Huffman
    let mut i = 0;
    while i < data.len() {
        let mut run = 0;
        if i > 0 {
            while i + run < data.len() && data[i + run] == data[i - 1] && run < 258 {
                run += 1;
            }
        }
        if run >= 3 {
            let code = BASE
                .iter()
                .rposition(|base| *base as usize <= run)
                .unwrap_or(0);
            bits.literal(257 + code as u32);
            bits.put(run as u32 - BASE[code], EXTRA[code]);
            bits.code(0, 5); // distance 1
            i += run;
        } else {
            bits.literal(u32::from(data[i]));
            i += 1;
        }
    }
    bits.literal(256);
    if bits.count > 0 {
        bits.out.push(bits.value as u8);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    bits.out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    bits.out
}

/// A palette PNG drawn by `pixel(x, y)`, which returns a palette index.
pub(crate) fn png(
    width: u32,
    height: u32,
    palette: &[[u8; 3]],
    pixel: impl Fn(u32, u32) -> u8,
) -> Vec<u8> {
    let mut rows = Vec::with_capacity(((width + 1) * height) as usize);
    let mut previous = vec![0u8; width as usize];
    for y in 0..height {
        rows.push(2); // Up filter
        let row: Vec<u8> = (0..width).map(|x| pixel(x, y)).collect();
        rows.extend(row.iter().zip(&previous).map(|(c, p)| c.wrapping_sub(*p)));
        previous = row;
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 3, 0, 0, 0]);
    chunk(b"IHDR", &header);
    chunk(b"PLTE", &palette.concat());
    chunk(b"IDAT", &zlib(&rows));
    chunk(b"IEND", &[]);
    out
}

fn ridge(x: u32, base: f32, peaks: &[(f32, f32, f32)]) -> f32 {
    let x = x as f32;
    peaks.iter().fold(base, |best, (center, height, width)| {
        best.min(base - (height - (x - center).abs() * width).max(0.0))
    })
}

/// A fjord at golden hour with a small red cabin, for the cabin-trip thread.
pub(crate) fn fjord_picture() -> Vec<u8> {
    let palette = [
        [250, 214, 165],
        [246, 190, 150],
        [236, 160, 140],
        [205, 132, 140], // sky bands
        [120, 98, 128],
        [86, 76, 110],
        [58, 62, 92], // far, mid, near ridges
        [64, 92, 124],
        [82, 116, 148],
        [232, 178, 140], // water, light water, sun path
        [176, 52, 46],
        [92, 32, 30],
        [244, 236, 220],
        [255, 236, 190], // cabin, roof, window, sun
        [44, 70, 54],
        [34, 54, 42], // shore, trees
    ];
    png(640, 400, &palette, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        if (fx - 470.0).powi(2) + (fy - 150.0).powi(2) < 900.0 {
            return 13;
        }
        let far = ridge(
            x,
            210.0,
            &[(120.0, 110.0, 0.9), (330.0, 70.0, 0.6), (560.0, 95.0, 0.8)],
        );
        let mid = ridge(
            x,
            240.0,
            &[(40.0, 90.0, 0.7), (250.0, 120.0, 1.1), (610.0, 80.0, 0.6)],
        );
        let near = ridge(x, 262.0, &[(-20.0, 140.0, 0.8), (640.0, 130.0, 0.7)]);
        if y < 250 {
            if fy >= near {
                return 6;
            }
            if fy >= mid {
                return 5;
            }
            if fy >= far {
                return 4;
            }
            return (y / 63).min(3) as u8;
        }
        if (280..330).contains(&x) && (300..336).contains(&y) {
            return if (296..312).contains(&x) && (310..322).contains(&y) {
                12
            } else {
                10
            };
        }
        if (272..338).contains(&x)
            && (282..300).contains(&y)
            && (fx - 305.0).abs() < 33.0 - (300.0 - fy) * 1.6
        {
            return 11;
        }
        if y > 318 {
            return if (x / 7 + y / 9) % 5 == 0 { 15 } else { 14 };
        }
        if (fx - 470.0).abs() < 22.0 - (fy - 250.0) * 0.12 && y % 6 < 3 {
            return 9;
        }
        if y % 14 < 2 { 8 } else { 7 }
    })
}

/// Northern lights over a ridge, the second cabin-trip picture.
pub(crate) fn aurora_picture() -> Vec<u8> {
    let palette = [
        [12, 20, 44],
        [18, 30, 62],
        [26, 44, 82], // night sky
        [60, 180, 130],
        [110, 220, 160],
        [40, 120, 110], // aurora
        [10, 14, 22],
        [230, 236, 250],
        [245, 200, 120],
        [28, 36, 40], // ridge, star, window, snow shadow
    ];
    png(640, 400, &palette, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let ground = ridge(x, 330.0, &[(180.0, 90.0, 0.5), (520.0, 120.0, 0.7)]);
        if fy >= ground {
            if (300..340).contains(&x) && (fy - ground) > 6.0 && (fy - ground) < 22.0 && x % 14 < 8
            {
                return 8;
            }
            return if y > 380 { 9 } else { 6 };
        }
        let wave = 130.0 + (fx / 70.0).sin() * 40.0 + (fx / 23.0).cos() * 8.0;
        let distance = fy - wave;
        if (-6.0..10.0).contains(&distance) {
            return 4;
        }
        if (-24.0..30.0).contains(&distance) {
            return if (x + y) % 3 == 0 { 5 } else { 3 };
        }
        if (x * 7919 + y * 104_729) % 997 == 0 {
            return 7;
        }
        (y / 110).min(2) as u8
    })
}

/// A light interface mockup for the design-review thread.
pub(crate) fn mockup_picture() -> Vec<u8> {
    let palette = [
        [248, 247, 244],
        [236, 233, 227],
        [255, 255, 255],
        [214, 210, 202], // page, sidebar, card, rule
        [42, 46, 54],
        [120, 124, 132],
        [74, 108, 247],
        [222, 230, 255], // ink, muted, accent, accent tint
        [246, 196, 92],  // highlight
    ];
    png(640, 400, &palette, |x, y| {
        let line = |x0: u32, x1: u32, y0: u32| (x0..x1).contains(&x) && (y0..y0 + 6).contains(&y);
        if x < 150 {
            if (20..130).contains(&x)
                && [40, 64, 88, 112]
                    .iter()
                    .any(|top| (*top..top + 14).contains(&y))
            {
                return if (40..54).contains(&y) { 7 } else { 3 };
            }
            return if x == 149 { 3 } else { 1 };
        }
        if (190..600).contains(&x) && (40..360).contains(&y) {
            if x == 190 || x == 599 || y == 40 || y == 359 {
                return 3;
            }
            if (230..290).contains(&x) && (80..140).contains(&y) {
                let (cx, cy) = (x as i32 - 260, y as i32 - 110);
                return if cx * cx + cy * cy < 900 { 8 } else { 2 };
            }
            if line(230, 470, 170) {
                return 4;
            }
            if line(230, 540, 196) || line(230, 500, 214) || line(230, 420, 232) {
                return 5;
            }
            if (230..360).contains(&x) && (280..316).contains(&y) {
                return 6;
            }
            if (376..480).contains(&x) && (280..316).contains(&y) {
                return if x == 376 || x == 479 || y == 280 || y == 315 {
                    3
                } else {
                    2
                };
            }
            return 2;
        }
        0
    })
}

/// A small coffee-roaster logo mark used as an inline `cid:` image.
pub(crate) fn roaster_logo() -> Vec<u8> {
    let palette = [[251, 246, 238], [111, 72, 45], [196, 140, 90], [60, 38, 24]];
    png(180, 56, &palette, |x, y| {
        let (dx, dy) = (x as f32 - 28.0, y as f32 - 28.0);
        let bean = (dx / 18.0).powi(2) + (dy / 24.0).powi(2);
        if bean < 1.0 {
            return if (dx + dy * 0.3).abs() < 2.0 { 3 } else { 1 };
        }
        if (60..170).contains(&x) && ((16..24).contains(&y) || ((32..38).contains(&y) && x < 140)) {
            return if y < 24 { 1 } else { 2 };
        }
        0
    })
}

// ---------------------------------------------------------------- PDF ------

/// A one-page PDF with Helvetica text lines `(size, x, y, text)` and rules.
pub(crate) fn pdf(lines: &[(u32, u32, u32, &str)], rules: &[(u32, u32, u32)]) -> Vec<u8> {
    let escape = |text: &str| {
        text.chars()
            .map(|c| match c {
                '(' | ')' | '\\' => format!("\\{c}"),
                c if c.is_ascii() => c.to_string(),
                // WinAnsi covers the Norwegian letters used in the demo.
                'æ' => "\\346".into(),
                'ø' => "\\370".into(),
                'å' => "\\345".into(),
                'Æ' => "\\306".into(),
                'Ø' => "\\330".into(),
                'Å' => "\\305".into(),
                '–' => "\\226".into(),
                '·' => "\\267".into(),
                '€' => "\\200".into(),
                _ => "?".into(),
            })
            .collect::<String>()
    };
    let mut content = String::new();
    for (size, x, y, text) in lines {
        content.push_str(&format!(
            "BT /F1 {size} Tf {x} {y} Td ({}) Tj ET\n",
            escape(text)
        ));
    }
    for (x0, x1, y) in rules {
        content.push_str(&format!("0.75 G 0.6 w {x0} {y} m {x1} {y} l S\n"));
    }
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
    ];
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// An invoice-style PDF: a header, address lines, a table of items and a total.
pub(crate) fn invoice_pdf(
    issuer: &str,
    issuer_lines: &[&str],
    title: &str,
    meta: &[(&str, String)],
    items: &[(&str, &str)],
    total: &str,
    footer: &str,
) -> Vec<u8> {
    let mut lines: Vec<(u32, u32, u32, String)> = vec![(20, 56, 776, issuer.into())];
    for (index, line) in issuer_lines.iter().enumerate() {
        lines.push((9, 56, 756 - index as u32 * 12, (*line).into()));
    }
    lines.push((16, 56, 680, title.into()));
    for (index, (key, value)) in meta.iter().enumerate() {
        lines.push((10, 56, 654 - index as u32 * 15, (*key).into()));
        lines.push((10, 200, 654 - index as u32 * 15, value.clone()));
    }
    let top = 654 - meta.len() as u32 * 15 - 30;
    lines.push((10, 56, top, "Description".into()));
    lines.push((10, 460, top, "Amount".into()));
    for (index, (item, amount)) in items.iter().enumerate() {
        let y = top - 26 - index as u32 * 18;
        lines.push((10, 56, y, (*item).into()));
        lines.push((10, 460, y, (*amount).into()));
    }
    let total_y = top - 40 - items.len() as u32 * 18;
    lines.push((12, 56, total_y, "Total".into()));
    lines.push((12, 460, total_y, total.into()));
    lines.push((8, 56, 60, footer.into()));
    let borrowed: Vec<_> = lines
        .iter()
        .map(|(s, x, y, t)| (*s, *x, *y, t.as_str()))
        .collect();
    pdf(&borrowed, &[(56, 539, top - 8), (56, 539, total_y + 14)])
}

// ---------------------------------------------------------------- ICS ------

pub(crate) struct Invite<'a> {
    pub uid: &'a str,
    pub method: &'a str,
    pub summary: &'a str,
    pub description: &'a str,
    pub location: &'a str,
    pub start_ms: i64,
    pub end_ms: i64,
    pub organizer: (&'a str, &'a str),
    pub attendees: &'a [(&'a str, &'a str)],
    pub rrule: Option<&'a str>,
}

pub(crate) fn utc_stamp(ms: i64) -> String {
    let (y, m, d, hh, mm) = super::clock::civil(ms);
    format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}00Z")
}

pub(crate) fn ics(invite: &Invite<'_>, now_ms: i64) -> Vec<u8> {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "PRODID:-//Fernhill Demo//Calendar//EN".into(),
        "VERSION:2.0".into(),
        format!("METHOD:{}", invite.method),
        "BEGIN:VEVENT".into(),
        format!("UID:{}", invite.uid),
        format!("DTSTAMP:{}", utc_stamp(now_ms)),
        format!("DTSTART:{}", utc_stamp(invite.start_ms)),
        format!("DTEND:{}", utc_stamp(invite.end_ms)),
        format!("SUMMARY:{}", ics_text(invite.summary)),
        format!("DESCRIPTION:{}", ics_text(invite.description)),
        format!("LOCATION:{}", ics_text(invite.location)),
        format!(
            "ORGANIZER;CN=\"{}\":mailto:{}",
            invite.organizer.0, invite.organizer.1
        ),
    ];
    for (name, email) in invite.attendees {
        lines.push(format!(
            "ATTENDEE;CN=\"{name}\";ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{email}"
        ));
    }
    if let Some(rule) = invite.rrule {
        lines.push(format!("RRULE:{rule}"));
    }
    lines.push(format!(
        "STATUS:{}",
        if invite.method == "CANCEL" {
            "CANCELLED"
        } else {
            "CONFIRMED"
        }
    ));
    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());
    let lines: Vec<String> = lines.iter().map(|line| fold(line)).collect();
    (lines.join("\r\n") + "\r\n").into_bytes()
}

/// RFC 5545 TEXT escaping.
fn ics_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

/// Fold a content line at 75 octets, never inside a UTF-8 character.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("Høstblend ☕".as_bytes()), "SMO4c3RibGVuZCDimJU=");
    }

    #[test]
    fn png_has_valid_chunks() {
        let image = roaster_logo();
        assert!(image.starts_with(b"\x89PNG\r\n\x1a\n"));
        let mut offset = 8;
        let mut kinds = Vec::new();
        while offset < image.len() {
            let length = u32::from_be_bytes(image[offset..offset + 4].try_into().unwrap()) as usize;
            let body = &image[offset + 4..offset + 8 + length];
            let crc = u32::from_be_bytes(
                image[offset + 8 + length..offset + 12 + length]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(crc32(body), crc);
            kinds.push(String::from_utf8_lossy(&body[..4]).to_string());
            offset += 12 + length;
        }
        assert_eq!(kinds, ["IHDR", "PLTE", "IDAT", "IEND"]);
        // Up filter plus runs keeps the illustrations small.
        assert!(fjord_picture().len() < 60_000);
    }

    #[test]
    fn invitations_are_escaped_folded_and_parse() {
        let description = "Agenda; one\\two, three\nNorsk: «møte» på fjellet ".repeat(4);
        let file = ics(
            &Invite {
                uid: "check@example.test",
                method: "REQUEST",
                summary: "Launch go/no-go",
                description: &description,
                location: "Room 1, Oslo",
                start_ms: 1_794_475_800_000,
                end_ms: 1_794_477_600_000,
                organizer: ("Ingrid Solberg", "ingrid@fernhill.example"),
                attendees: &[("Alex Morgan", "alex@fernhill.example")],
                rrule: None,
            },
            1_794_475_800_000,
        );
        let text = String::from_utf8(file.clone()).unwrap();
        assert!(text.split("\r\n").all(|line| line.len() <= 75));
        let events = crate::parse_calendar_invitations(&file).unwrap();
        assert_eq!(events[0].description, description);
        assert_eq!(events[0].location, "Room 1, Oslo");
        assert_eq!(
            events[0].organizer.as_ref().unwrap().name.as_deref(),
            Some("Ingrid Solberg")
        );
    }

    #[test]
    fn pdf_xref_points_at_objects() {
        let file = invoice_pdf(
            "Issuer",
            &["Street 1"],
            "Invoice",
            &[("No.", "1".into())],
            &[("Item", "1.00")],
            "1.00",
            "Footer",
        );
        let text = String::from_utf8_lossy(&file);
        let start: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(file[start..].starts_with(b"xref"));
        let table = String::from_utf8_lossy(&file[start..]).to_string();
        for (index, line) in table.lines().skip(3).take(5).enumerate() {
            let offset: usize = line[..10].parse().unwrap();
            assert!(file[offset..].starts_with(format!("{} 0 obj", index + 1).as_bytes()));
        }
    }
}
