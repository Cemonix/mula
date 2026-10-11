//! Putting text on the system clipboard through the terminal, with OSC 52.

use std::io::{self, Write};

/// Writes the sequence asking the terminal to put `text` on the system
/// clipboard, and flushes it. The terminal answers nothing: one that does not
/// know the sequence, or is set to refuse it, drops it without a word.
pub fn copy(text: &[u8], out: &mut impl Write) -> io::Result<()> {
    write!(out, "\x1b]52;c;{}\x07", base64(text))?;
    out.flush()
}

/// `bytes` in standard base64, padded with `=`.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |group, (i, byte)| {
            group | (u32::from(*byte) << (16 - 8 * i))
        });
        // Three bytes make four characters; each byte short of three leaves
        // one of them as padding.
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(
                    ALPHABET[((group >> (18 - 6 * i)) & 0x3f) as usize],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    /// The test vectors of RFC 4648, section 10.
    #[test]
    fn base64_matches_the_rfc() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded, "{plain:?}");
        }
    }

    #[test]
    fn every_bit_of_a_byte_survives() {
        assert_eq!(base64(&[0xff, 0x00, 0x80]), "/wCA");
    }

    #[test]
    fn the_text_goes_out_as_one_osc_52_sequence() {
        let mut out = Vec::new();

        copy(b"/tmp/a b", &mut out).unwrap();

        assert_eq!(out, b"\x1b]52;c;L3RtcC9hIGI=\x07");
    }
}
