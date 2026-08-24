//! The pictures the terminal holds of its own, and the only place that writes
//! the sequences putting them there.
//!
//! A placement outlives the frame it was made in: the terminal keeps it and
//! goes on drawing it under whatever text arrives later. Ratatui knows nothing
//! about it — it diffs cells, and no cell of a picture it never drew ever
//! changes — so a placement left alone would sit under the next file's name.
//!
//! So the screen is reconciled rather than drawn: what should be on it against
//! what is, the same shape as the diff ratatui does over `Buffer`, one level
//! up. Two passes wanting the same picture in the same place write nothing,
//! which is what keeps a bitmap from crossing sixty times a second.

use std::io::{self, Write};

use ratatui::layout::Rect;

use crate::{
    fs::preview::Bitmap,
    ui::graphics::{
        capabilities::{Capabilities, Graphics, Protocol},
        kitty,
    },
};

/// A picture on the screen: which one, and where.
///
/// Compared whole, so a picture that moved, that was replaced, and one that
/// went away are one comparison rather than three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Which answer from the reader this picture is. A number rather than a
    /// path: the same path read twice is two pictures, and the second one has
    /// to reach the screen.
    pub image: u64,
    /// The cells it covers.
    pub area: Rect,
}

/// A picture the panel wants on the screen this pass, with the pixels that go
/// there.
#[derive(Debug)]
pub struct Wanted<'a> {
    pub placement: Placement,
    pub bitmap: &'a Bitmap,
}

/// What the terminal is holding, and the only thing that tells it to hold
/// anything else.
#[derive(Debug)]
pub struct Surface {
    graphics: Option<Graphics>,
    /// What the terminal was last told to put on the screen.
    placed: Option<Placement>,
}

impl Surface {
    /// A screen holding nothing, over a terminal that can draw what
    /// `capabilities` says it can.
    pub fn new(capabilities: Capabilities) -> Self {
        Self {
            graphics: capabilities.graphics(),
            placed: None,
        }
    }

    /// What the panel needs to lay a picture out, or `None` where it should
    /// draw half blocks instead.
    pub fn graphics(&self) -> Option<Graphics> {
        self.graphics
    }

    /// Brings the screen in line with `wanted`, writing nothing at all when it
    /// already is.
    ///
    /// Runs after the frame has been drawn: the terminal anchors a placement
    /// where the cursor stands, and lays it over the text that is already
    /// there.
    pub fn reconcile(
        &mut self,
        wanted: Option<Wanted<'_>>,
        out: &mut impl Write,
    ) -> io::Result<()> {
        match self.graphics {
            Some(graphics) => self.reconcile_with(&graphics.protocol, wanted, out),
            None => Ok(()),
        }
    }

    /// Takes back everything on the screen. Leaving the alternate screen does
    /// not do it: a placement belongs to the terminal, not to the screen it
    /// was made on.
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.reconcile(None, out)
    }

    /// The reconciling itself, over whatever writes the sequences.
    fn reconcile_with(
        &mut self,
        encoder: &impl Encode,
        wanted: Option<Wanted<'_>>,
        out: &mut impl Write,
    ) -> io::Result<()> {
        let desired = wanted.as_ref().map(|wanted| wanted.placement);
        if desired == self.placed {
            return Ok(());
        }

        // Given up before anything is written, so a write that fails partway
        // leaves nothing claimed that may not be there.
        if self.placed.take().is_some() {
            encoder.forget(out)?;
        }

        if let Some(wanted) = wanted {
            encoder.place(wanted.bitmap, wanted.placement.area, out)?;
            self.placed = Some(wanted.placement);
        }

        out.flush()
    }
}

/// Takes back every picture on the screen, whatever put it there.
///
/// Belongs to no `Surface`: it is for the way out of a panic, where the one
/// that made the placements cannot be reached and what is on the screen is no
/// longer known.
pub fn forget_all(protocol: Protocol, out: &mut impl Write) -> io::Result<()> {
    match protocol {
        Protocol::Kitty => kitty::forget_all(out)?,
    }

    out.flush()
}

/// Turns a change on the screen into the bytes that carry it out.
trait Encode {
    /// Puts `bitmap` on the screen over `area`, scaled to fill it.
    fn place(&self, bitmap: &Bitmap, area: Rect, out: &mut dyn Write) -> io::Result<()>;

    /// Takes back whatever this last put there.
    fn forget(&self, out: &mut dyn Write) -> io::Result<()>;
}

impl Encode for Protocol {
    fn place(&self, bitmap: &Bitmap, area: Rect, out: &mut dyn Write) -> io::Result<()> {
        match self {
            Protocol::Kitty => kitty::place(bitmap, area, out),
        }
    }

    fn forget(&self, out: &mut dyn Write) -> io::Result<()> {
        match self {
            Protocol::Kitty => kitty::forget(out),
        }
    }
}

#[cfg(test)]
mod surface_tests {
    use std::num::NonZeroU16;

    use super::*;

    use crate::ui::graphics::capabilities::CellSize;

    /// An encoder that writes down what it was asked to do rather than any
    /// protocol, so the reconciling can be read on its own.
    struct Recorder;

    impl Encode for Recorder {
        fn place(&self, bitmap: &Bitmap, area: Rect, out: &mut dyn Write) -> io::Result<()> {
            write!(out, "place {} at {},{};", bitmap.width, area.x, area.y)
        }

        fn forget(&self, out: &mut dyn Write) -> io::Result<()> {
            write!(out, "forget;")
        }
    }

    fn graphics() -> Graphics {
        let side = NonZeroU16::new(10).expect("ten is not zero");
        Graphics {
            protocol: Protocol::Kitty,
            cell: CellSize {
                width: side,
                height: side,
            },
        }
    }

    fn bitmap(width: u32) -> Bitmap {
        Bitmap::of(width, 1, vec![0; (width * 4) as usize])
    }

    fn placement(image: u64, x: u16) -> Placement {
        Placement {
            image,
            area: Rect::new(x, 0, 4, 4),
        }
    }

    /// Reconciles each of `passes` in turn and hands back everything written
    /// over all of them.
    fn run(passes: &[Option<(Placement, u32)>]) -> String {
        let mut surface = Surface {
            graphics: Some(graphics()),
            placed: None,
        };
        let mut out = Vec::new();

        for pass in passes {
            let held = pass.map(|(placement, width)| (placement, bitmap(width)));
            let wanted = held.as_ref().map(|(placement, bitmap)| Wanted {
                placement: *placement,
                bitmap,
            });

            surface
                .reconcile_with(&Recorder, wanted, &mut out)
                .expect("a vector never fails to be written to");
        }

        String::from_utf8(out).expect("the recorder writes text")
    }

    #[test]
    fn a_picture_reaches_a_screen_holding_nothing() {
        assert_eq!(run(&[Some((placement(1, 0), 8))]), "place 8 at 0,0;");
    }

    /// The one that matters: a picture already on the screen is not sent
    /// again, however many passes of the loop go by.
    #[test]
    fn the_same_picture_in_the_same_place_is_sent_once() {
        let wanted = Some((placement(1, 0), 8));
        assert_eq!(run(&[wanted, wanted, wanted]), "place 8 at 0,0;");
    }

    #[test]
    fn a_new_picture_is_forgotten_before_the_next_is_placed() {
        let passes = [Some((placement(1, 0), 8)), Some((placement(2, 0), 9))];
        assert_eq!(run(&passes), "place 8 at 0,0;forget;place 9 at 0,0;");
    }

    /// A picture that only moved is still a placement the terminal holds where
    /// it should not.
    #[test]
    fn a_picture_that_moved_is_placed_again() {
        let passes = [Some((placement(1, 0), 8)), Some((placement(1, 3), 8))];
        assert_eq!(run(&passes), "place 8 at 0,0;forget;place 8 at 3,0;");
    }

    /// What an overlay opening over the panel, and Quick View closing, both
    /// come to.
    #[test]
    fn wanting_nothing_takes_the_picture_back() {
        let passes = [Some((placement(1, 0), 8)), None];
        assert_eq!(run(&passes), "place 8 at 0,0;forget;");
    }

    #[test]
    fn a_screen_already_holding_nothing_is_left_alone() {
        assert_eq!(run(&[None, None]), "");
    }

    /// The same path read twice is two pictures. Only the number tells them
    /// apart, and the second one has to reach the screen.
    #[test]
    fn a_second_reading_of_one_path_is_a_second_picture() {
        let passes = [Some((placement(1, 0), 8)), Some((placement(2, 0), 8))];
        assert_eq!(run(&passes), "place 8 at 0,0;forget;place 8 at 0,0;");
    }

    #[test]
    fn a_terminal_that_draws_no_pictures_is_written_nothing() {
        let mut surface = Surface::new(Capabilities::default());
        let mut out = Vec::new();
        let bitmap = bitmap(8);

        surface
            .reconcile(
                Some(Wanted {
                    placement: placement(1, 0),
                    bitmap: &bitmap,
                }),
                &mut out,
            )
            .expect("a vector never fails to be written to");

        assert!(out.is_empty());
    }
}
