//! The system clipboard: ctrl-v as a paste key where the terminal does
//! not bind it itself, and the transcript selection's copy. A fresh
//! handle per use, the connection is not held open between presses.
//!
//! Desktop sessions write through `arboard`; a terminal-only session
//! (a pipe, an ssh hop, a TTY with no X11 or Wayland display) has no
//! clipboard to connect to, so the copy falls back to OSC 52 and lets
//! the terminal put the text on its own clipboard.

use base64::Engine;
use std::io::Write as _;

/// Writes `text` to the clipboard, best effort. A reachable desktop
/// clipboard takes it directly; otherwise the terminal is asked with
/// OSC 52, which is also what carries a copy over ssh. The terminal
/// silently ignores the sequence when it does not support it, so there
/// is no failure to report.
pub fn write(text: &str) {
    if arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .is_ok()
    {
        return;
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout
        .write_all(osc52(text).as_bytes())
        .and_then(|()| stdout.flush());
}

/// The OSC 52 sequence that asks the terminal to put `text` on its
/// clipboard: the payload is the UTF-8 bytes as base64.
fn osc52(text: &str) -> String {
    format!(
        "\x1b]52;c;{}\x1b\\",
        base64::engine::general_purpose::STANDARD.encode(text)
    )
}

/// The clipboard's text, when one is reachable.
pub fn read() -> Option<String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.get_text())
        .ok()
        .filter(|text| !text.is_empty())
}

/// What one read of the clipboard found: the paste key's answer
/// distinguished by cause, because a clipboard holding nothing is
/// silence and no clipboard at all is a notice.
pub enum Grab {
    /// The clipboard's image: the RGBA pixels re-encoded as PNG, the
    /// one format every provider accepts and the neutral carrier for
    /// whatever the copy came from, as bare base64, the form
    /// [`rhema_core::Image`] carries.
    Image(rhema_core::Image),
    /// The clipboard's text.
    Text(String),
    /// The clipboard's image past the per-image ceiling every provider
    /// accepts, the encoded PNG's byte length, refused here the same
    /// way the see staging refuses at its CLI, one limit for both
    /// routes in. The paste key answers with a notice naming it.
    Oversized(usize),
    /// A reachable clipboard held neither: no image, and no text.
    Nothing,
    /// No clipboard to connect to: an ssh hop, a plain TTY, anywhere
    /// without an X11 or Wayland display. The terminal's own paste
    /// still carries text; an image has no way over the hop, and the
    /// paste key says so instead of staying silent.
    Unreachable,
}

/// The notice a paste with no clipboard to read lands in the
/// transcript: no clipboard is an environment fact, not an error, and
/// the file way in is the answer that works over the hop.
pub const UNREACHABLE_NOTICE: &str = "no clipboard reachable; an image cannot ride a paste: save it in the checkout and name it in the prompt";

/// Reads the clipboard for one paste key. The image is tried first, a
/// clipboard holds one thing or the other, then the text. The one
/// connection serves both reads, and a dead one answers
/// [`Grab::Unreachable`], the case the paste key answers with a
/// notice: [`Grab::Nothing`] is an empty clipboard, not a missing
/// one.
pub fn grab() -> Grab {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(_) => return Grab::Unreachable,
    };
    if let Ok(grabbed) = clipboard.get_image()
        && let Some((png_bytes, image)) = encode(grabbed)
    {
        // The ceiling the see path enforces at staging, enforced at
        // read for a clipboard image, the only other way an image
        // enters a message: past it the provider refuses the whole
        // request, better named at the paste key.
        if png_bytes > rhema_bash::vision::MAX_IMAGE_BYTES {
            return Grab::Oversized(png_bytes);
        }
        return Grab::Image(image);
    }
    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => Grab::Text(text),
        _ => Grab::Nothing,
    }
}

/// The image read's encode tail: arboard's RGBA pixels in, the
/// encoded PNG's byte length and its base64 out, callable without a
/// clipboard. The pixels must fill their frame, `width * height * 4`,
/// or the shape is a lie and encodes as nothing. The byte length is
/// the plain-encoding size the ceiling reads, measured before the
/// base64 covers it.
fn encode(grabbed: arboard::ImageData) -> Option<(usize, rhema_core::Image)> {
    let (width, height): (u32, u32) = match (grabbed.width.try_into(), grabbed.height.try_into()) {
        (Ok(width), Ok(height)) => (width, height),
        // A frame the pixel types do not agree on encodes as nothing.
        _ => return None,
    };
    let rgba = image::RgbaImage::from_raw(width, height, grabbed.bytes.into_owned())?;
    let mut png = Vec::new();
    rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    let bytes = png.len();
    let image = rhema_core::Image::new(
        rhema_core::ImageMediaType::Png,
        base64::engine::general_purpose::STANDARD.encode(png),
    );
    Some((bytes, image))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clipboard itself has no test double: what is testable here is
    /// the pure tail of the read, RGBA pixels in, base64 PNG out, the
    /// exact form a message block carries.
    #[test]
    fn a_copy_encodes_as_an_osc52_sequence() {
        assert_eq!(osc52("hello"), "\x1b]52;c;aGVsbG8=\x1b\\");
    }

    #[test]
    fn rgba_pixels_encode_as_bare_base64_png() {
        let (bytes, image) = encode_png(1, 1, vec![255, 0, 0, 255]).unwrap();
        assert_eq!(image.media_type, rhema_core::ImageMediaType::Png);
        assert!(
            !image.data.contains("data:"),
            "bare base64, no data: prefix: {}",
            image.data
        );
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .unwrap();
        // The encoding is real PNG, by the same magic-byte rule a
        // block's media_type answers to.
        assert_eq!(
            rhema_core::ImageMediaType::sniff(&decoded),
            Some(rhema_core::ImageMediaType::Png)
        );
        // The byte length is the plain encoding's, not the base64's,
        // the size the ceiling reads.
        assert_eq!(bytes, decoded.len());
    }

    /// Pixels that do not fill their frame encode as nothing, the
    /// length must match `width * height * 4`, or the shape is a lie.
    #[test]
    fn a_short_pixel_buffer_encodes_as_none() {
        assert!(encode_png(2, 2, vec![0, 0, 0, 0]).is_none());
    }

    /// The pure tail of encode, callable without a clipboard.
    fn encode_png(width: u32, height: u32, rgba: Vec<u8>) -> Option<(usize, rhema_core::Image)> {
        let rgba = image::RgbaImage::from_raw(width, height, rgba)?;
        let mut png = Vec::new();
        rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        let bytes = png.len();
        let image = rhema_core::Image::new(
            rhema_core::ImageMediaType::Png,
            base64::engine::general_purpose::STANDARD.encode(png),
        );
        Some((bytes, image))
    }
}
