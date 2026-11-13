//! Terminal graphics for the onboarding logo and the transcript's
//! bubble arcs: the kitty graphics protocol, which Ghostty and WezTerm
//! speak too.
//!
//! Placement happens OUTSIDE the frame: ratatui paints cells, and a
//! graphics-protocol image is a separate layer composited above them,
//! so drawing a frame never erases one. [`place`] is called after a
//! draw, with the terminal cursor moved to the cell the image anchors
//! on; the caller re-places when the anchor rect moves (a resize) and
//! calls [`clear`] when the image's screen goes away. Leaving the
//! alternate screen discards the images in conforming terminals, but
//! `clear` runs on teardown anyway — a terminal that keeps them must
//! not show a stray logo over the user's shell.
//!
//! The logo escape is transmit-and-display (`a=T`): the PNG payload
//! rides the escape, and `c`/`r` scale it to the cell area given. The
//! payload is chunked (`m=1`) at the protocol's recommended chunk size,
//! so a 45 KB logo crosses in a handful of writes a pipe absorbs
//! instantly. The prompt card works the other way around — five
//! sprites (the [`Corner`] arcs and the solid fill) transmit once and
//! get placed by id ([`transmit_sprite`], [`place_piece`]), because a
//! scrolled transcript re-states its placements every frame and the
//! payloads must not move with it.

use std::io::{IsTerminal, Write};

/// The PNG the binary carries, copied from the repository's images by
/// `build.rs` — empty in a packaged build without the asset, where
/// [`Self::logo_png`] answers `None` and the caller falls back to the
/// text wordmark.
const LOGO_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rhema-logo.png"));

/// The logo asset, when the binary carries one.
pub fn logo_png() -> Option<&'static [u8]> {
    (!LOGO_PNG.is_empty()).then_some(LOGO_PNG)
}

/// The base64 chunk the graphics protocol recommends: larger chunks are
/// legal but may overflow terminal input buffers.
const CHUNK: usize = 4096;

/// Whether `TERM` names Ghostty's own terminfo entry. Ghostty sets it
/// for every shell it starts, and unlike `TERM_PROGRAM` and
/// `GHOSTTY_RESOURCES_DIR` it survives ssh — the way a remote session
/// reaches a Ghostty terminal at all.
fn ghostty_term(term: Option<&str>) -> bool {
    matches!(term, Some(term) if term == "ghostty" || term.ends_with("-ghostty"))
}

/// Whether `TERM` names WezTerm's terminfo entry, the mark that
/// survives ssh the same way Ghostty's does.
fn wezterm_term(term: Option<&str>) -> bool {
    term == Some("wezterm")
}

/// Whether this terminal draws the kitty graphics protocol — kitty,
/// Ghostty, and WezTerm, by their own environment marks — and the
/// output is a terminal at all. `RHEMA_NO_IMAGES` overrides detection
/// off: an escape a terminal mislabels as garbage on screen is worse
/// than no image. Inside tmux the answer is no: tmux swallows the
/// protocol (no passthrough by default), so an outer kitty's marks
/// would promise pixels the inner grid never receives.
pub fn graphics() -> bool {
    if std::env::var_os("RHEMA_NO_IMAGES").is_some() || std::env::var_os("TMUX").is_some() {
        return false;
    }
    if !std::io::stdout().is_terminal() {
        return false;
    }
    let term = std::env::var("TERM").ok();
    let program_name = std::env::var("TERM_PROGRAM").ok();
    let program = program_name.as_deref();
    let kitty = std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("KITTY_PID").is_some()
        || term.as_deref() == Some("xterm-kitty");
    let ghostty = std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || program == Some("ghostty")
        || ghostty_term(term.as_deref());
    let wezterm = std::env::var_os("WEZTERM_EXECUTABLE").is_some()
        || std::env::var_os("WEZTERM_PANE").is_some()
        || program == Some("wezterm")
        || wezterm_term(term.as_deref());
    kitty || ghostty || wezterm
}

/// Whether the terminal draws the prompt card through the protocol:
/// the graphics protocol is available, and the terminal honors the
/// text layer for negative-z placements. Kitty and Ghostty composite
/// a `z=-1` image under the glyphs; WezTerm composites it over the
/// glyphs (its tracker: "kitty image not blending with text"), where
/// the card would wash the prompt it should back — so there the card
/// stays off and the frame keeps its see-through interior. The card
/// itself ships translucent, so a terminal that ignores the layer
/// anyway washes the prompt instead of hiding it.
pub fn card_graphics() -> bool {
    graphics() && !wezterm_term(std::env::var("TERM").ok().as_deref())
}

/// The card's opacity: the fill and the arcs bake it into their alpha,
/// so the card tints whatever sits behind it — the palette's own page
/// under `background: rhema`, the terminal's ground under `native`.
/// It also keeps the failure mode above soft: a card drawn over the
/// text veils it rather than covering it.
pub const CARD_ALPHA: u8 = 150;

/// One card edge: the fill's strip under a hairline, sized to the
/// inner half of the cell the hairline hollows out, plus the whole
/// interior. Every bubble's placements reference these ids, so a
/// scrolled transcript re-states its placements without moving a
/// payload. The arcs carry [`Corner::sprite_id`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The interior between the edges: tinted through.
    Fill,
    /// The cap band between the top arcs: tinted below the hairline.
    Top,
    /// The foot band between the bottom arcs: tinted above it.
    Bottom,
    /// The left band between the top and bottom arcs: tinted right of
    /// the hairline.
    Left,
    /// The right band: tinted left of it.
    Right,
}

impl Edge {
    /// All five, in transmission order.
    pub const ALL: [Edge; 5] = [Edge::Fill, Edge::Top, Edge::Bottom, Edge::Left, Edge::Right];

    /// The sprite id this edge transmits under.
    pub fn sprite_id(self) -> u32 {
        SPRITE_BASE + 4 + self as u32
    }
}

/// Whether the onboarding logo can draw: the graphics protocol, and an
/// image to draw.
pub fn supported() -> bool {
    graphics() && logo_png().is_some()
}

/// Deletes every image this program placed. Safe on terminals without
/// the protocol: an unknown APC sequence is ignored by design.
pub fn clear() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b_Ga=d,d=a,q=2\x1b\\");
    let _ = out.flush();
}

/// Places the logo at `x`/`y` (cell column/row, zero-based), scaled to
/// `cols` cells wide and `rows` cells tall. Any image placed before is
/// deleted first, so a re-place never stacks two logos. Failures are
/// swallowed: a logo that cannot draw is a missing decoration, not an
/// error worth a screen.
pub fn place(x: u16, y: u16, cols: u16, rows: u16) {
    let Some(png) = logo_png() else { return };

    let encoded = base64(png);
    let mut out = std::io::stdout();
    // The image anchors on the cell the terminal cursor sits at when
    // the escape lands; the cursor itself does not move.
    let _ = out.write_all(format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
    clear();
    write_chunks(
        &format!("a=T,f=100,c={cols},r={rows},q=2"),
        &encoded,
        &mut out,
    );
    let _ = out.flush();
}

/// One escape's chunked body: the base64 payload rides `m=1` chunks of
/// [`CHUNK`], only the first carrying `head` (a continuation chunk is a
/// bare transmit — the data continues the previous command).
fn write_chunks(head: &str, encoded: &str, out: &mut impl Write) {
    let total = encoded.len();
    let mut offset = 0;
    while offset < total {
        let end = (offset + CHUNK).min(total);
        let more = end < total;
        let _ = out.write_all(b"\x1b_G");
        if offset == 0 {
            let _ = out.write_all(head.as_bytes());
        } else {
            let _ = out.write_all(b"a=t,q=2");
        }
        let _ = out.write_all(if more { ",m=1;" } else { ";" }.as_bytes());
        let _ = out.write_all(&encoded.as_bytes()[offset..end]);
        let _ = out.write_all(b"\x1b\\");
        offset = end;
    }
}

/// The sprite ids start high: a transmit without an id draws
/// auto-assigned ones from 1, and a collision would replace another
/// image's data under it.
const SPRITE_BASE: u32 = 1000;

/// How far the notification card's sprite set sits above the prompt
/// set: nine sprites each (four corners, five edges), so a transcript
/// holding both cards transmits both sets and their placements never
/// share an image id.
pub const NOTICE_SPRITE_OFFSET: u32 = 9;

/// One card corner: the quarter-disc a bubble arc sprite draws. The
/// four corners transmit once per fill color — a few kilobytes — and
/// every bubble's placements reference its corners' ids, so a scrolled
/// transcript re-states its placements without moving a payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    /// The card's top-left cell.
    TopLeft,
    /// The card's top-right cell.
    TopRight,
    /// The card's bottom-left cell.
    BottomLeft,
    /// The card's bottom-right cell.
    BottomRight,
}

impl Corner {
    /// Whether the pixel sits in the sprite's inner half — the side
    /// the hairlines bound. The frame's glyphs draw through the middle
    /// of their cells, so a card painted to the cell edge would
    /// overhang the drawn frame by that half on every side; the tint
    /// occupies only the inner quarter of each corner cell and the
    /// card's edge lands on the drawn frame instead.
    pub fn in_inner_half(self, x: u16, y: u16) -> bool {
        let x_inner = match self {
            Corner::TopLeft | Corner::BottomLeft => x * 2 >= SPRITE_W,
            Corner::TopRight | Corner::BottomRight => x * 2 < SPRITE_W,
        };
        let y_inner = match self {
            Corner::TopLeft | Corner::TopRight => y * 2 >= SPRITE_H,
            Corner::BottomLeft | Corner::BottomRight => y * 2 < SPRITE_H,
        };
        x_inner && y_inner
    }

    /// All four, in transmission order.
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    /// The sprite id this corner transmits under.
    pub fn sprite_id(self) -> u32 {
        SPRITE_BASE + self as u32
    }

    /// The quarter-ellipse's center in the sprite's pixel grid — the
    /// card's interior corner, the point the curve pulls away from:
    /// inside the ellipse the fill paints, outside it the cell's own
    /// ground shows through.
    fn center(self) -> (f32, f32) {
        match self {
            Corner::TopLeft => (SPRITE_W as f32, SPRITE_H as f32),
            Corner::TopRight => (0.0, SPRITE_H as f32),
            Corner::BottomLeft => (SPRITE_W as f32, 0.0),
            Corner::BottomRight => (0.0, 0.0),
        }
    }
}

/// The sprite's cell, in pixels: two per column and per row of a
/// common terminal cell, so the terminal's fit-to-cell stretch lands
/// near 1:1. The 1:2 shape matches the cell aspect, so the
/// quarter-ellipse reads circular again after the stretch.
const SPRITE_W: u16 = 24;
const SPRITE_H: u16 = 48;

/// Coverage subsamples per pixel axis: a pixel's alpha is the share of
/// its samples inside the ellipse, the graded edge no block glyph
/// carries.
const SUBSAMPLES: u32 = 6;

/// Builds one corner sprite: `fill`'s quarter-ellipse over a
/// transparent ground, anti-aliased along the curve. The bubble's own
/// cells paint the fill and the page's ground; the sprite lays over
/// the corner cell only, painting inside the curve and leaving the
/// cell's ground visible outside it, so the arc blends against
/// whatever ground the mode runs — the terminal's own included.
pub fn corner_sprite(corner: Corner, fill: (u8, u8, u8)) -> Vec<u8> {
    let (cx, cy) = corner.center();
    // The curve runs from the mid-point of one hairline to the
    // mid-point of the other — half a cell out from the interior
    // corner, so the rounded edge tracks the glyph arcs instead of
    // sweeping across the whole cell.
    let (rx, ry) = (SPRITE_W as f32 / 2.0, SPRITE_H as f32 / 2.0);
    let samples = SUBSAMPLES * SUBSAMPLES;
    let mut rgba = Vec::with_capacity(SPRITE_W as usize * SPRITE_H as usize * 4);
    for y in 0..SPRITE_H {
        for x in 0..SPRITE_W {
            let mut alpha = 0u32;
            if corner.in_inner_half(x, y) {
                let mut inside = 0u32;
                for sample in 0..samples {
                    let sx = (sample % SUBSAMPLES) as f32 + 0.5;
                    let sy = (sample / SUBSAMPLES) as f32 + 0.5;
                    let dx = (x as f32 + sx / SUBSAMPLES as f32 - cx) / rx;
                    let dy = (y as f32 + sy / SUBSAMPLES as f32 - cy) / ry;
                    if dx * dx + dy * dy <= 1.0 {
                        inside += 1;
                    }
                }
                // The card's opacity rides the coverage: the arc stays
                // anti-aliased and translucent with it. The quadrant
                // cut is binary, so the flat edges against the
                // hairlines stay hard.
                alpha = inside * CARD_ALPHA as u32 / samples;
            }
            rgba.extend_from_slice(&[fill.0, fill.1, fill.2, alpha as u8]);
        }
    }
    png_rgba(SPRITE_W, SPRITE_H, &rgba)
}

/// One card edge's sprite: the tint on the inner side of the hairline
/// (the [`Edge::Fill`] interior through), bare where the frame's
/// glyphs hollow the cell. Uniform color stretches to any cell count
/// without distortion, and it rides the card's opacity like the arcs
/// do. The edge is hard, not graded: it sits under the hairline ink,
/// where a fade would read as mist.
pub fn edge_sprite(edge: Edge, fill: (u8, u8, u8)) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(SPRITE_W as usize * SPRITE_H as usize * 4);
    for y in 0..SPRITE_H {
        for x in 0..SPRITE_W {
            let tinted = match edge {
                Edge::Fill => true,
                Edge::Top => y * 2 >= SPRITE_H,
                Edge::Bottom => y * 2 < SPRITE_H,
                Edge::Left => x * 2 >= SPRITE_W,
                Edge::Right => x * 2 < SPRITE_W,
            };
            let alpha = if tinted { CARD_ALPHA } else { 0 };
            rgba.extend_from_slice(&[fill.0, fill.1, fill.2, alpha]);
        }
    }
    png_rgba(SPRITE_W, SPRITE_H, &rgba)
}

/// Transmits one sprite's PNG under `id` — the data crosses once, and
/// [`place_piece`] references it by id afterwards.
pub fn transmit_sprite(id: u32, png: &[u8]) {
    let encoded = base64(png);
    let mut out = std::io::stdout();
    write_chunks(&format!("a=t,f=100,i={id},q=2"), &encoded, &mut out);
    let _ = out.flush();
}

/// Places a transmitted sprite at `x`/`y` (cell column/row,
/// zero-based), scaled to `cols`×`rows` cells, under the placement id
/// `slot`, below the text layer (`z=-1`): the card backs the frame's
/// glyphs and the prompt text instead of covering them. A placement
/// is the pair of the image id and the slot: restating the pair moves
/// and resizes the piece in place — the protocol replaces it, no
/// flicker and no delete — so scrolling the transcript only re-states
/// slots. Failures are
pub fn place_piece(id: u32, slot: u16, x: u16, y: u16, cols: u16, rows: u16) {
    let mut out = std::io::stdout();
    let _ = out.write_all(format!("\x1b[{};{}H", y + 1, x + 1).as_bytes());
    let _ = out.write_all(
        format!("\x1b_Ga=p,i={id},p={slot},c={cols},r={rows},z=-1,q=2\x1b\\").as_bytes(),
    );
    let _ = out.flush();
}

/// Removes one placement: the image's `slot`. The sprites' data stays
/// transmitted, and every other placement — the logo's included — is
/// untouched. Failures are ignored: a missing placement deletes as
/// nothing.
pub fn delete_placement(id: u32, slot: u16) {
    let mut out = std::io::stdout();
    let _ = out.write_all(format!("\x1b_Ga=d,d=I,i={id},p={slot},q=2\x1b\\").as_bytes());
    let _ = out.flush();
}

/// Encodes RGBA pixels (row-major, 4 bytes a pixel) as a PNG. No codec
/// dependency: the zlib stream stores the scanlines uncompressed —
/// deflate's stored blocks — which every decoder reads, and a corner
/// cell of pixels costs less than a keystroke to transmit. The sprites
/// travel once a fill color; placements reference them by id.
fn png_rgba(width: u16, height: u16, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        width as usize * height as usize * 4,
        "the pixels fill the canvas"
    );
    // The scanlines: one None-filter byte per row, what a decoder
    // un- filters for free.
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[row * stride..(row + 1) * stride]);
    }
    // zlib: the 2-byte stream header, the stored deflate blocks (the
    // one-byte block header carries BFINAL; LEN and its complement
    // follow), then the Adler-32 of the uncompressed bytes.
    let mut z = vec![0x78u8, 0x01];
    let blocks = raw.chunks(0xFFFF);
    let count = raw.chunks(0xFFFF).count();
    for (index, block) in blocks.enumerate() {
        z.push(u8::from(index + 1 == count));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = Vec::with_capacity(raw.len() + 64);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    // Bit depth 8, RGBA, deflate, an adaptive-free filter pass, no
    // interlace: the simplest legal truecolor-with-alpha header.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_chunk(&mut png, b"IHDR", &ihdr);
    write_chunk(&mut png, b"IDAT", &z);
    write_chunk(&mut png, b"IEND", &[]);
    png
}

/// One PNG chunk: the length, the type, the data, and the CRC-32 over
/// type and data.
fn write_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut signed = Vec::with_capacity(4 + data.len());
    signed.extend_from_slice(kind);
    signed.extend_from_slice(data);
    png.extend_from_slice(&crc32(&signed).to_be_bytes());
}

/// CRC-32, the polynomial PNG chunks sign with, bit at a time — three
/// calls a sprite, and the table is not worth its space.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Adler-32, the checksum a zlib stream ends with.
fn adler32(data: &[u8]) -> u32 {
    let (mut low, mut high) = (1u32, 0u32);
    for byte in data {
        low = (low + u32::from(*byte)) % 65521;
        high = (high + low) % 65521;
    }
    (high << 16) | low
}

/// Standard base64 — the graphics protocol's payload alphabet. Written
/// here rather than pulled in as a dependency: one table, one padding
/// rule, no configuration.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let byte = |i: usize| chunk.get(i).copied().unwrap_or(0) as u32;
        let group = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        out.push(ALPHABET[(group >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(group >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(group >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[group as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_reference_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_ghostty_mark_survives_ssh() {
        // `TERM` crosses ssh; `TERM_PROGRAM` and the resources dir do not.
        assert!(ghostty_term(Some("xterm-ghostty")));
        assert!(ghostty_term(Some("ghostty")));
        assert!(!ghostty_term(Some("xterm-kitty")));
        assert!(!ghostty_term(Some("xterm-256color")));
        assert!(!ghostty_term(None));
    }

    #[test]
    fn the_wezterm_mark_survives_ssh() {
        assert!(wezterm_term(Some("wezterm")));
        assert!(!wezterm_term(Some("xterm-256color")));
        assert!(!wezterm_term(None));
    }

    #[test]
    fn the_embedded_payload_is_a_png_or_none() {
        if let Some(bytes) = logo_png() {
            assert!(bytes.len() > 8, "a real asset, not a stray file");
            assert_eq!(&bytes[..4], b"\x89PNG", "the PNG magic");
        }
    }

    /// Decodes the stored-block PNGs this module writes: the signature,
    /// the chunks, the zlib pass-through, the None filters. A decoder
    /// this small exists because the encoder chose the one branch of
    /// each format a test can walk back through.
    fn decode_png(png: &[u8]) -> (u16, u16, Vec<u8>) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "the PNG signature");
        let mut index = 8;
        let mut dims = None;
        let mut idat = Vec::new();
        while index < png.len() {
            let len = u32::from_be_bytes(png[index..index + 4].try_into().unwrap()) as usize;
            let kind = &png[index + 4..index + 8];
            let data = &png[index + 8..index + 8 + len];
            match kind {
                b"IHDR" => dims = Some((len, data.to_vec())),
                b"IDAT" => idat.extend_from_slice(data),
                _ => {}
            }
            index += 12 + len;
        }
        let (len, ihdr) = dims.expect("an IHDR chunk");
        assert_eq!(len, 13, "the IHDR payload");
        assert_eq!(ihdr[8..11], [8, 6, 0], "depth 8, RGBA, deflate");
        let width = u32::from_be_bytes(ihdr[0..4].try_into().unwrap()) as u16;
        let height = u32::from_be_bytes(ihdr[4..8].try_into().unwrap()) as u16;

        // The zlib stream: header, stored blocks, the Adler-32 tail.
        let mut raw = Vec::new();
        let mut cursor = 2;
        loop {
            let header = idat[cursor];
            let block_len =
                u16::from_le_bytes(idat[cursor + 1..cursor + 3].try_into().unwrap()) as usize;
            assert_eq!(
                !block_len as u16,
                u16::from_le_bytes(idat[cursor + 3..cursor + 5].try_into().unwrap()),
                "LEN and NLEN are complements"
            );
            cursor += 5;
            raw.extend_from_slice(&idat[cursor..cursor + block_len]);
            cursor += block_len;
            if header & 1 == 1 {
                break;
            }
        }
        assert_eq!(cursor + 4, idat.len(), "the Adler-32 tail");
        let mut sum = 1u32;
        let mut weight = 0u32;
        for byte in &raw {
            sum = (sum + u32::from(*byte)) % 65521;
            weight = (weight + sum) % 65521;
        }
        assert_eq!(
            (weight << 16) | sum,
            u32::from_be_bytes(idat[cursor..cursor + 4].try_into().unwrap()),
            "the stream checksum"
        );

        // The scanlines: a None filter byte each.
        let stride = width as usize * 4 + 1;
        let mut rgba = Vec::with_capacity(stride * height as usize);
        for row in 0..height as usize {
            assert_eq!(raw[row * stride], 0, "no scanline filters");
            rgba.extend_from_slice(&raw[row * stride + 1..(row + 1) * stride]);
        }
        (width, height, rgba)
    }

    #[test]
    fn the_corner_sprite_is_a_quarter_disc() {
        // TopRight's tint lives in its bottom-left quarter — the side
        // the hairlines bound — at the card's opacity near the interior
        // corner, bare across the outer half, and graded along the arc.
        let (width, height, rgba) = decode_png(&corner_sprite(Corner::TopRight, (1, 2, 3)));
        assert_eq!((width, height), (SPRITE_W, SPRITE_H));
        let alpha = |x: u16, y: u16| rgba[(y as usize * width as usize + x as usize) * 4 + 3];
        assert_eq!(alpha(SPRITE_W - 1, 0), 0, "the outer corner is bare");
        assert_eq!(
            alpha(0, SPRITE_H - 1),
            CARD_ALPHA,
            "the interior corner fills at the card's opacity"
        );
        assert_eq!(
            alpha(SPRITE_W - 1, SPRITE_H / 2 - 1),
            0,
            "everything above the hairline stays bare"
        );
        assert_eq!(
            alpha(SPRITE_W / 2 + 1, SPRITE_H - 1),
            0,
            "so does everything right of it"
        );
        // The arc runs from the left hairline's mid-point to the top
        // one; a pixel on it grades through the card's opacity.
        let edge = alpha(8, 31);
        assert!(
            (1..CARD_ALPHA).contains(&edge),
            "the curve grades through {edge}"
        );
        // The fill rides every painted pixel, and only the alpha grades.
        for pixel in rgba.chunks(4) {
            assert_eq!(&pixel[..3], &[1, 2, 3], "the fill color");
        }
    }

    #[test]
    fn the_card_stops_at_the_hairlines() {
        // Each corner tints only its inner quarter: the outer halves —
        // the cell the frame's glyphs hollow out — stay ground, and the
        // quarter's far pixel, the one toward the card's body, fills.
        for corner in Corner::ALL {
            let (_, _, rgba) = decode_png(&corner_sprite(corner, (0, 0, 0)));
            let alpha =
                |x: u16, y: u16| rgba[(y as usize * SPRITE_W as usize + x as usize) * 4 + 3];
            let right = SPRITE_W - 1;
            let bottom = SPRITE_H - 1;
            // The outer corner, plus the two extremes of the outer
            // half along each hairline: all ground.
            let bare = match corner {
                Corner::TopLeft => [(0, 0), (0, bottom), (right, 0)],
                Corner::TopRight => [(right, 0), (right, bottom), (0, 0)],
                Corner::BottomLeft => [(0, bottom), (0, 0), (right, bottom)],
                Corner::BottomRight => [(right, bottom), (right, 0), (0, bottom)],
            };
            for (x, y) in bare {
                assert_eq!(alpha(x, y), 0, "{corner:?}'s outer half at ({x},{y})");
            }
            // The inner quarter's far pixel fills: the disc reaches it.
            let full = match corner {
                Corner::TopLeft => (right, bottom),
                Corner::TopRight => (0, bottom),
                Corner::BottomLeft => (right, 0),
                Corner::BottomRight => (0, 0),
            };
            assert_eq!(
                alpha(full.0, full.1),
                CARD_ALPHA,
                "{corner:?}'s interior pixel"
            );
        }
    }

    #[test]
    fn the_arc_meets_the_edges_it_runs_into() {
        // TopLeft's arc lands where the strips pick it up: the bottom
        // row's tint begins at the hairline (the left strip's rule),
        // and the right column's begins at it (the top strip's rule).
        let (_, _, rgba) = decode_png(&corner_sprite(Corner::TopLeft, (0, 0, 0)));
        let alpha = |x: u16, y: u16| rgba[(y as usize * SPRITE_W as usize + x as usize) * 4 + 3];
        assert_eq!(
            alpha(SPRITE_W / 2 - 1, SPRITE_H - 1),
            0,
            "the last bare pixel before the left strip takes over"
        );
        assert!(
            alpha(SPRITE_W / 2, SPRITE_H - 1) > 0,
            "the tint is running at the seam"
        );
        assert!(
            alpha(SPRITE_W - 1, SPRITE_H / 2) > 0,
            "and along the top seam too"
        );
    }

    #[test]
    fn every_edge_pads_the_hollowed_half() {
        // Each strip tints its inner half at the card's opacity and
        // bares the half the hairline hollows out; the interior tints
        // through.
        for edge in Edge::ALL {
            let (width, height, rgba) = decode_png(&edge_sprite(edge, (4, 5, 6)));
            assert_eq!((width, height), (SPRITE_W, SPRITE_H));
            let alpha = |x: u16, y: u16| rgba[(y as usize * width as usize + x as usize) * 4 + 3];
            let right = SPRITE_W - 1;
            let bottom = SPRITE_H - 1;
            let (bare, full) = match edge {
                Edge::Fill => (
                    None,
                    Some(vec![(0, 0), (right, bottom), (0, bottom), (right, 0)]),
                ),
                Edge::Top => (Some((0, 0)), Some(vec![(0, bottom), (right, bottom)])),
                Edge::Bottom => (Some((0, bottom)), Some(vec![(0, 0), (right, 0)])),
                Edge::Left => (Some((0, 0)), Some(vec![(right, 0), (right, bottom)])),
                Edge::Right => (Some((right, 0)), Some(vec![(0, 0), (0, bottom)])),
            };
            if let Some((x, y)) = bare {
                assert_eq!(alpha(x, y), 0, "{edge:?}'s hollowed half at ({x},{y})");
            }
            if let Some(points) = full {
                for (x, y) in points {
                    assert_eq!(alpha(x, y), CARD_ALPHA, "{edge:?}'s tint at ({x},{y})");
                }
            }
            for pixel in rgba.chunks(4) {
                assert_eq!(&pixel[..3], &[4, 5, 6], "{edge:?}'s fill color");
            }
        }
    }

    #[test]
    fn every_corner_pulls_toward_its_own_interior() {
        // Each sprite fills the quarter its card sits in and bares the
        // diagonally opposite pixel.
        let bare = |corner| {
            let (_, _, rgba) = decode_png(&corner_sprite(corner, (0, 0, 0)));
            let alpha =
                |x: u16, y: u16| rgba[(y as usize * SPRITE_W as usize + x as usize) * 4 + 3];
            let right = SPRITE_W - 1;
            let bottom = SPRITE_H - 1;
            match corner {
                Corner::TopLeft => alpha(0, 0),
                Corner::TopRight => alpha(right, 0),
                Corner::BottomLeft => alpha(0, bottom),
                Corner::BottomRight => alpha(right, bottom),
            }
        };
        for corner in Corner::ALL {
            assert_eq!(bare(corner), 0, "{corner:?}'s outer pixel");
        }
    }
}
