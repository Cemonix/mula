//! The kitty graphics protocol: a bitmap crosses as base64 inside APC
//! sequences and the terminal keeps it as a placement of its own.
//!
//! Everything here writes bytes and nothing here reads any: the terminal is
//! told to stay quiet with `q=2`, so its answers cannot land in the middle of
//! the key loop and be taken for keystrokes.

use std::io::{self, Write};

use ratatui::layout::Rect;

use crate::fs::preview::Bitmap;

/// The image and the placement this makes. One picture is on the screen at a
/// time, so one number does for both and a delete never has to say which.
const IMAGE: u32 = 1;

/// The most base64 one sequence carries. The protocol caps a chunk at 4096
/// bytes and wants every chunk but the last to be a multiple of four, which
/// this is.
const CHUNK: usize = 4096;

/// The alphabet the payload is spelled in, and the byte that pads a short
/// group out to four characters.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PAD: u8 = b'=';

/// Puts `bitmap` on the screen over `area`, scaled to fill it.
///
/// The cursor is moved first: a placement is anchored wherever the cursor
/// stands when the sequence arrives. `C=1` leaves it there afterwards, so what
/// ratatui believes about the screen stays true.
///
/// `a=T` transmits and places in one go, `f=32` says the payload is the four
/// bytes a pixel this carries, and `c`/`r` give the terminal the cells to
/// scale into so it does the fitting itself.
pub fn place(bitmap: &Bitmap, area: Rect, out: &mut dyn Write) -> io::Result<()> {
    let payload = base64(bitmap.rgba());
    let mut chunks = payload.chunks(CHUNK);

    // A bitmap with no pixels has no first chunk, and nothing to place.
    let Some(first) = chunks.next() else {
        return Ok(());
    };

    // Rows and columns count from one in `CUP`.
    write!(out, "\x1b[{};{}H", area.y + 1, area.x + 1)?;

    let mut rest = chunks.peekable();
    write!(
        out,
        "\x1b_Ga=T,f=32,s={},v={},i={IMAGE},p={IMAGE},c={},r={},q=2,C=1,m={};",
        bitmap.width,
        bitmap.height,
        area.width,
        area.height,
        more(rest.peek().is_some()),
    )?;
    out.write_all(first)?;
    out.write_all(b"\x1b\\")?;

    // Every chunk after the first carries only whether another follows; the
    // control data was settled by the one that opened the transmission.
    while let Some(chunk) = rest.next() {
        write!(out, "\x1b_Gq=2,m={};", more(rest.peek().is_some()))?;
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }

    Ok(())
}

/// Takes the picture back off the screen. The capital `I` frees the pixels
/// with it; the next placement brings its own.
pub fn forget(out: &mut dyn Write) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={IMAGE},q=2\x1b\\")
}

/// Takes back every picture the terminal is showing, named or not.
///
/// `d=A` is what a caller reaches for when it cannot say what is on the
/// screen, which on the way out of a panic nothing can.
pub fn forget_all(out: &mut dyn Write) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=A,q=2\x1b\\")
}

/// The `m` a chunk carries: one while more of the payload follows, zero on the
/// one that ends it.
fn more(follows: bool) -> u8 {
    u8::from(follows)
}

/// Standard base64 with padding, which is how the protocol spells a payload.
fn base64(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);

    for group in bytes.chunks(3) {
        // The group as one number, with the bytes it does not have left zero.
        let bits = group.iter().enumerate().fold(0u32, |bits, (at, byte)| {
            bits | u32::from(*byte) << (16 - 8 * at)
        });

        // Four characters of six bits each, of which a short group fills only
        // as many as it has whole or partial bytes for.
        for at in 0..4 {
            out.push(match at <= group.len() {
                true => ALPHABET[(bits >> (18 - 6 * at) & 0x3f) as usize],
                false => PAD,
            });
        }
    }

    out
}

#[cfg(test)]
mod kitty_tests {
    use super::*;

    /// A bitmap of `width` by `height` whose bytes count up, so a payload can
    /// be checked against what went in.
    fn bitmap(width: u32, height: u32) -> Bitmap {
        let pixels = (0..width * height * 4).map(|byte| byte as u8).collect();
        Bitmap::of(width, height, pixels)
    }

    fn placed(bitmap: &Bitmap, area: Rect) -> Vec<u8> {
        let mut out = Vec::new();
        place(bitmap, area, &mut out).expect("a vector never fails to be written to");
        out
    }

    /// Base64 of every length of tail, against the answers the standard gives.
    #[test]
    fn a_payload_is_spelled_the_way_the_standard_spells_it() {
        assert_eq!(base64(b""), b"");
        assert_eq!(base64(b"M"), b"TQ==");
        assert_eq!(base64(b"Ma"), b"TWE=");
        assert_eq!(base64(b"Man"), b"TWFu");
        assert_eq!(base64(b"Mula!"), b"TXVsYSE=");
        assert_eq!(base64(&[0xff, 0xff, 0xff]), b"////");
        assert_eq!(base64(&[0x00, 0x00, 0x00]), b"AAAA");
    }

    #[test]
    fn the_payload_carries_the_pixels_it_was_given() {
        let bitmap = bitmap(4, 4);
        let written = placed(&bitmap, Rect::new(0, 0, 4, 2));

        let written = String::from_utf8(written).expect("the sequence is text");
        // The payload follows the first semicolon *of the APC sequence*; the
        // cursor move ahead of it carries one of its own.
        let payload = written
            .split_once("\x1b_G")
            .expect("a placement opens an APC sequence")
            .1
            .split_once(';')
            .expect("the payload follows the control data")
            .1
            .trim_end_matches("\x1b\\");

        assert_eq!(payload.as_bytes(), base64(bitmap.rgba()));
    }

    #[test]
    fn the_sequence_says_how_big_the_bitmap_is_and_what_it_fills() {
        let written = placed(&bitmap(6, 3), Rect::new(9, 4, 20, 10));
        let written = String::from_utf8(written).expect("the sequence is text");

        assert!(written.contains("s=6,v=3"), "{written}");
        assert!(written.contains("c=20,r=10"), "{written}");
    }

    /// A placement lands where the cursor is, and `CUP` counts from one.
    #[test]
    fn the_cursor_is_moved_to_the_corner_of_the_area_first() {
        let written = placed(&bitmap(2, 2), Rect::new(9, 4, 20, 10));
        assert!(written.starts_with(b"\x1b[5;10H"), "{written:?}");
    }

    /// The chunking is what a bitmap of any size rests on: none over the cap,
    /// and the last one saying it is the last.
    #[test]
    fn a_payload_is_broken_into_chunks_the_protocol_accepts() {
        // Four bytes a pixel over 64 by 64 is 16384, whose base64 needs six
        // chunks.
        let written = placed(&bitmap(64, 64), Rect::new(0, 0, 20, 10));
        let written = String::from_utf8(written).expect("the sequence is text");

        let payloads: Vec<&str> = written
            .split("\x1b_G")
            .skip(1)
            .map(|sequence| {
                sequence
                    .split_once(';')
                    .expect("every sequence has a payload")
                    .1
                    .trim_end_matches("\x1b\\")
            })
            .collect();

        assert!(payloads.len() > 1, "one chunk is not a chunked payload");
        assert!(payloads.iter().all(|chunk| chunk.len() <= CHUNK));
        assert_eq!(
            payloads.concat(),
            String::from_utf8_lossy(&base64(bitmap(64, 64).rgba()))
        );

        let last = written.rfind("\x1b_G").expect("there is a last sequence");
        assert!(
            written[last..].starts_with("\x1b_Gq=2,m=0;"),
            "{}",
            &written[last..last + 20]
        );
    }

    /// A payload short enough for one sequence still has to say it is over.
    #[test]
    fn a_single_chunk_is_marked_as_the_last() {
        let written = placed(&bitmap(2, 2), Rect::new(0, 0, 4, 2));
        let written = String::from_utf8(written).expect("the sequence is text");

        assert!(written.contains("m=0;"), "{written}");
        assert_eq!(written.matches("\x1b_G").count(), 1);
    }

    #[test]
    fn a_bitmap_with_no_pixels_places_nothing() {
        assert!(placed(&bitmap(0, 0), Rect::new(0, 0, 4, 2)).is_empty());
    }

    #[test]
    fn forgetting_names_the_image_it_frees() {
        let mut out = Vec::new();
        forget(&mut out).expect("a vector never fails to be written to");
        assert_eq!(out, b"\x1b_Ga=d,d=I,i=1,q=2\x1b\\");
    }

    /// The panic path cannot say what is on the screen, so it names nothing.
    #[test]
    fn forgetting_everything_names_no_image_at_all() {
        let mut out = Vec::new();
        forget_all(&mut out).expect("a vector never fails to be written to");
        assert_eq!(out, b"\x1b_Ga=d,d=A,q=2\x1b\\");
    }
}
