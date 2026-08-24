//! What the terminal can draw, asked once before the loop starts.
//!
//! The answers arrive on standard input, so this runs while the terminal is in
//! raw mode and before anything else reads the keyboard: an event reader would
//! take them for keystrokes.
//!
//! Three questions go out in one write. The last is a Primary Device
//! Attributes request, which every VT-compatible terminal answers, so its
//! arrival ends the read without waiting out a timeout. A terminal that
//! ignores one of the other two sends nothing for it and the rest still lands
//! where this can find it: each answer is matched by its own shape rather than
//! by its place in the queue.

use std::{
    io::{self, Write},
    num::NonZeroU16,
    time::{Duration, Instant},
};

use rustix::termios::{self, OptionalActions, SpecialCodeIndex};

/// The three questions, in one write.
///
/// `CSI 16 t` asks how big a cell is. The APC sequence offers the kitty
/// protocol a single RGB pixel under `a=q`, which asks about it and neither
/// stores nor places it, so a terminal that understands the sequence draws
/// nothing. `CSI c` is the fence.
const QUESTIONS: &[u8] = b"\x1b[16t\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

/// Final byte of the Device Attributes reply, which is the fence.
const ATTRIBUTES: u8 = b'c';
/// Final byte of the window-op reply that carries the cell size.
const WINDOW_OP: u8 = b't';
/// The parameter a `CSI 16 t` reply leads with. `CSI 14 t` ends in the same
/// byte and leads with a `4`, and measures the whole window instead.
const CELL_SIZE: &[u8] = b"6";

/// How long one read waits before it comes back empty-handed, in the
/// deciseconds `VTIME` counts in.
const SILENCE: u8 = 3;
/// How long the whole round trip may take, however talkative the terminal is.
const MAX_WAIT: Duration = Duration::from_millis(500);
/// Room for the three answers and then some. A terminal still talking past
/// this is answering something nobody asked.
const MAX_REPLY: usize = 1024;

/// A way of putting a picture on the screen that the panel knows how to write.
///
/// Names what this can do rather than what terminals can: one the panel cannot
/// write is one the panel must not choose, or it would blank the half blocks
/// and put nothing in their place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// Kitty graphics: the bitmap crosses as base64 inside an APC sequence and
    /// the terminal keeps it as a placement of its own.
    Kitty,
}

/// The size of one cell in pixels, which is what turns an area measured in
/// cells into one measured in pixels.
///
/// Neither side can be zero, so dividing an area by one is always an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSize {
    pub width: NonZeroU16,
    pub height: NonZeroU16,
}

/// What the terminal answered. Both halves are missing from a terminal that
/// says nothing, which is where half blocks are the answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub protocol: Option<Protocol>,
    pub cell: Option<CellSize>,
}

/// What it takes to place a picture: a protocol to carry it and the size of a
/// cell to lay it out in.
///
/// Held together rather than as two answers either of which could be missing,
/// so the panel asks once and gets everything or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Graphics {
    pub protocol: Protocol,
    pub cell: CellSize,
}

impl Capabilities {
    /// Asks the terminal what it can draw.
    ///
    /// Standard input is left in the mode it was found in. A terminal that
    /// cannot be asked at all — standard input is not one, or it answers
    /// nothing — comes back as half blocks.
    pub fn detect() -> Self {
        match ask() {
            Ok(reply) => {
                tracing::debug!(reply = %reply.escape_ascii(), "terminal answered");
                Self::from_reply(&reply)
            }
            Err(error) => {
                tracing::debug!(%error, "the terminal could not be asked");
                Self::default()
            }
        }
    }

    /// Reads everything the terminal said out of one buffer.
    fn from_reply(reply: &[u8]) -> Self {
        Self {
            protocol: kitty(reply).then_some(Protocol::Kitty),
            cell: cell_size(reply),
        }
    }

    /// What it takes to place a picture, or `None` where half blocks are the
    /// answer: a protocol without a cell size cannot keep a picture's shape,
    /// and a cell size without a protocol has nothing to lay out.
    pub fn graphics(self) -> Option<Graphics> {
        Some(Graphics {
            protocol: self.protocol?,
            cell: self.cell?,
        })
    }
}

/// Puts the questions on the wire and gathers whatever comes back.
fn ask() -> io::Result<Vec<u8>> {
    let stdin = io::stdin();
    let saved = termios::tcgetattr(&stdin)?;

    // A read on standard input blocks until a key arrives, and a terminal that
    // answers nothing would never send one. `VMIN` of zero with a `VTIME` lets
    // a read come back empty instead.
    let mut timed = saved.clone();
    timed.special_codes[SpecialCodeIndex::VMIN] = 0;
    timed.special_codes[SpecialCodeIndex::VTIME] = SILENCE;
    termios::tcsetattr(&stdin, OptionalActions::Now, &timed)?;

    let reply = listen(&stdin);

    // The mode belongs to the terminal rather than to this process, so it goes
    // back whether anything was answered or not.
    termios::tcsetattr(&stdin, OptionalActions::Now, &saved)?;
    reply
}

/// Writes the questions and reads until the fence answers, the terminal falls
/// silent, or it has said more than any answer could be.
///
/// Reads the descriptor rather than `Stdin`, whose buffer would hold on to
/// bytes that arrived after the fence and keep them from the key loop.
fn listen(stdin: &io::Stdin) -> io::Result<Vec<u8>> {
    let mut stdout = io::stdout();
    stdout.write_all(QUESTIONS)?;
    stdout.flush()?;

    let deadline = Instant::now() + MAX_WAIT;
    let mut reply = Vec::new();
    let mut chunk = [0; 64];

    while reply.len() < MAX_REPLY && Instant::now() < deadline {
        match rustix::io::read(stdin, &mut chunk) {
            // `VMIN` of zero turns a read that waited out its `VTIME` into an
            // empty one, which is the terminal saying nothing more is coming.
            Ok(0) => break,
            Ok(read) => reply.extend_from_slice(&chunk[..read]),
            Err(error) if error == rustix::io::Errno::INTR => continue,
            Err(error) => return Err(error.into()),
        }

        if csi_params(&reply, ATTRIBUTES).is_some() {
            break;
        }
    }

    Ok(reply)
}

/// Whether the kitty query came back with an `OK`. The body of the answer is
/// `i=<id>;OK` when the protocol is there and `i=<id>;<error>` when the
/// terminal understood the sequence but not the request.
fn kitty(reply: &[u8]) -> bool {
    apc_body(reply).is_some_and(|body| body.ends_with(b";OK"))
}

/// The cell size out of a `CSI 6 ; height ; width t` reply, which puts the
/// height first. A zero in either is a terminal reporting that it does not
/// know, and is turned away as no answer at all.
fn cell_size(reply: &[u8]) -> Option<CellSize> {
    let mut params = csi_params(reply, WINDOW_OP)?.split(|byte| *byte == b';');
    if params.next()? != CELL_SIZE {
        return None;
    }

    let height = NonZeroU16::new(number(params.next()?)?)?;
    let width = NonZeroU16::new(number(params.next()?)?)?;
    Some(CellSize { width, height })
}

/// The body of the first APC sequence in `reply`: what sits between `ESC _ G`
/// and the `ESC \` closing it.
fn apc_body(reply: &[u8]) -> Option<&[u8]> {
    let start = find(reply, b"\x1b_G")? + 3;
    let rest = &reply[start..];
    let end = find(rest, b"\x1b\\")?;
    Some(&rest[..end])
}

/// The parameters of the first `CSI` sequence in `reply` that ends in
/// `final_byte`: everything between `ESC [` and that byte.
///
/// A sequence runs over parameter bytes (`0x30..=0x3f`) and intermediates
/// (`0x20..=0x2f`); the first byte outside both ends it and says which
/// sequence it was. One that ends in another byte is stepped over, so the
/// answers can be picked out of a reply that holds several.
fn csi_params(reply: &[u8], final_byte: u8) -> Option<&[u8]> {
    let mut at = 0;

    while let Some(found) = find(&reply[at..], b"\x1b[") {
        let start = at + found + 2;
        let Some(offset) = reply[start..]
            .iter()
            .position(|byte| !matches!(byte, 0x20..=0x3f))
        else {
            // The sequence runs past what has arrived, and nothing after an
            // unfinished one can be complete either.
            return None;
        };

        let end = start + offset;
        if reply[end] == final_byte {
            return Some(&reply[start..end]);
        }
        at = end + 1;
    }

    None
}

/// A decimal parameter, or `None` where it is empty or not a number.
fn number(bytes: &[u8]) -> Option<u16> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

/// Where `needle` first sits in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a terminal with the kitty protocol says to the query.
    const KITTY_OK: &[u8] = b"\x1b_Gi=31;OK\x1b\\";
    /// What one that knows the sequence but refuses the request says.
    const KITTY_ERROR: &[u8] = b"\x1b_Gi=31;ENOTSUPPORTED:aq\x1b\\";
    /// Device Attributes, which every terminal answers and which is the fence.
    const ATTRIBUTES_PLAIN: &[u8] = b"\x1b[?62;22c";
    /// A cell eight pixels across and seventeen tall, height first.
    const CELL_REPLY: &[u8] = b"\x1b[6;17;8t";

    fn reply(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    fn cell(width: u16, height: u16) -> CellSize {
        CellSize {
            width: NonZeroU16::new(width).expect("a test cell has a width"),
            height: NonZeroU16::new(height).expect("a test cell has a height"),
        }
    }

    #[test]
    fn a_terminal_that_says_nothing_draws_half_blocks() {
        assert_eq!(Capabilities::from_reply(b""), Capabilities::default());
    }

    #[test]
    fn an_ok_to_the_query_is_the_kitty_protocol() {
        let answered = reply(&[KITTY_OK, ATTRIBUTES_PLAIN]);
        assert_eq!(
            Capabilities::from_reply(&answered).protocol,
            Some(Protocol::Kitty)
        );
    }

    #[test]
    fn an_error_to_the_query_is_not() {
        let answered = reply(&[KITTY_ERROR, ATTRIBUTES_PLAIN]);
        assert_eq!(Capabilities::from_reply(&answered).protocol, None);
    }

    /// Both halves have to be there before anything can be laid out.
    #[test]
    fn a_protocol_without_a_cell_size_places_nothing() {
        let answered = reply(&[KITTY_OK, ATTRIBUTES_PLAIN]);
        assert_eq!(Capabilities::from_reply(&answered).graphics(), None);
    }

    #[test]
    fn a_cell_size_without_a_protocol_places_nothing() {
        let answered = reply(&[CELL_REPLY, ATTRIBUTES_PLAIN]);
        assert_eq!(Capabilities::from_reply(&answered).graphics(), None);
    }

    #[test]
    fn the_cell_size_comes_out_width_first() {
        assert_eq!(cell_size(CELL_REPLY), Some(cell(8, 17)));
    }

    /// `CSI 14 t` ends in the same byte and answers a different question.
    #[test]
    fn the_window_size_is_not_the_cell_size() {
        assert_eq!(cell_size(b"\x1b[4;1080;1920t"), None);
    }

    #[test]
    fn a_cell_of_no_size_is_no_answer() {
        assert_eq!(cell_size(b"\x1b[6;0;0t"), None);
    }

    /// The answers arrive in one buffer, and the fence has to be found past a
    /// sequence ending in another byte.
    #[test]
    fn every_answer_out_of_one_reply() {
        let answered = reply(&[CELL_REPLY, KITTY_OK, ATTRIBUTES_PLAIN]);
        assert_eq!(
            Capabilities::from_reply(&answered).graphics(),
            Some(Graphics {
                protocol: Protocol::Kitty,
                cell: cell(8, 17),
            })
        );
    }

    /// Everything up to the fence may be missing, which is what a terminal
    /// answering only what it knows leaves behind.
    #[test]
    fn the_fence_alone_is_an_answer() {
        assert_eq!(
            Capabilities::from_reply(ATTRIBUTES_PLAIN),
            Capabilities::default()
        );
    }

    #[test]
    fn a_sequence_that_never_ends_is_not_read_past() {
        assert_eq!(csi_params(b"\x1b[62;4", ATTRIBUTES), None);
    }
}
