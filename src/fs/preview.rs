//! Reading whatever is under the cursor so the opposite panel can show it.
//!
//! Runs on a [`Reader`](crate::fs::reader::Reader) of its own rather than
//! beside the search: the two are told apart by their generations, and sharing
//! one counter would only hold together because the find overlay happens to be
//! modal today.
//!
//! Unlike a search, a preview is a *snapshot*. Running through ten entries
//! leaves nine answers that are not merely useless but wrong — an image drawn
//! under another file's name. Only the newest one is ever folded in.

use std::{
    fmt,
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use image::{ImageFormat, ImageReader};

use crate::fs::{
    directory::Directory,
    reader::{Live, Outbox, ReadJob},
};

/// One file to look at, taken when the request was sent. Like a `Job`, it
/// never reads back.
#[derive(Debug)]
pub struct Preview {
    pub path: Arc<Path>,
}

/// The bounds a preview keeps to. They describe the reader rather than one
/// question asked of it, so they are fixed when the thread starts.
#[derive(Clone, Debug)]
pub struct Limits {
    /// How much of a file is read at all. The cap is in bytes and not in
    /// lines, because a file without line endings is one line.
    pub max_bytes: usize,
    /// Lines kept from a text file. A panel shows tens of them; the rest is
    /// read and dropped rather than carried around.
    pub max_lines: usize,
    /// Characters kept from one line, so a minified bundle on a single line
    /// cannot be the thing that is measured and wrapped every frame.
    pub max_line_chars: usize,
    /// The largest file that is decoded as an image at all. Unlike text, an
    /// image has to be read whole, so the cap is what stands between a preview
    /// and a hundred megabytes of it.
    pub max_image_bytes: u64,
    /// The longest side a stored image may claim before it is refused
    /// undecoded, which is what a decompression bomb runs into.
    pub max_source_side: u32,
    /// The longest side of the bitmap that is carried back. A panel is tens of
    /// cells across, so anything beyond this is detail nothing can draw.
    pub max_bitmap_side: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024,
            max_lines: 500,
            max_line_chars: 512,
            max_image_bytes: 32 * 1024 * 1024,
            max_source_side: 20_000,
            max_bitmap_side: 512,
        }
    }
}

/// A decoded image, cut down to something worth carrying across a channel and
/// sampling from every frame.
///
/// Kept as RGBA rather than composited: what shows through a transparent pixel
/// is whatever the panel sits on, and only the widget knows that.
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Four bytes to a pixel, row after row.
    pixels: Vec<u8>,
}

/// Prints the size rather than the pixels, so a bitmap can sit in a `Debug`
/// struct without a megabyte coming out of it.
impl fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bitmap")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Bitmap {
    /// The pixel at `(x, y)` as red, green, blue and alpha. Outside the
    /// bitmap it is transparent black, so a caller that samples a box need not
    /// clamp its edges.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }

        let at = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[at],
            self.pixels[at + 1],
            self.pixels[at + 2],
            self.pixels[at + 3],
        ]
    }

    /// The pixels as they are stored, for a protocol that carries a bitmap
    /// whole rather than sampling it a point at a time.
    pub fn rgba(&self) -> &[u8] {
        &self.pixels
    }

    /// A bitmap of `width` by `height` from raw RGBA bytes.
    #[cfg(test)]
    pub fn of(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert_eq!(pixels.len(), (width * height * 4) as usize);
        Self {
            width,
            height,
            pixels,
        }
    }
}

/// Why there is nothing to show. Kept as a reason rather than as a sentence,
/// so the wording lives with the widget that draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// A pipe, a socket or a device. Opening one can block forever, so it is
    /// turned away on its metadata and never opened.
    NotAFile,
}

/// What a symlink points at, resolved once so the widget need not touch the
/// disk to word it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Directory,
    File,
    Other,
    /// Nothing is there. A symlink is shown by where it points either way.
    Broken,
}

/// What the panel draws. Everything here is settled: no path is opened again
/// and no decision is left to the widget.
#[derive(Debug)]
pub enum Content {
    Directory(Directory),
    Symlink {
        target: PathBuf,
        points_to: LinkTarget,
    },
    Text {
        lines: Vec<String>,
        /// Set when the file goes on past what was read or kept.
        clipped: bool,
    },
    /// Bytes as they were read. The dump is laid out by the widget, which is
    /// the only thing that knows how wide a row may be.
    Binary {
        bytes: Vec<u8>,
        clipped: bool,
    },
    Image(Bitmap),
    Refused(Refused),
    /// Permissions, a file that vanished between the listing and the read. A
    /// normal state of a file manager, drawn in the panel rather than raised
    /// as a toast.
    Unreadable(String),
}

impl ReadJob for Preview {
    type Config = Limits;
    type Msg = Content;

    fn run(self, limits: &Limits, live: &Live<'_>, out: &Outbox<'_, Content>) {
        if let Some(content) = read(&self.path, limits, live) {
            out.send(content);
        }
    }
}

/// Reads `path` far enough to draw it, or gives up with `None` once the cursor
/// has moved on and the answer would be thrown away anyway.
///
/// The kind of the path is settled from its metadata before anything is
/// opened: a fifo or a device would block `File::open` for as long as nobody
/// writes to it, and the thread would never come back.
fn read(path: &Path, limits: &Limits, live: &Live<'_>) -> Option<Content> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) => return Some(Content::Unreadable(e.to_string())),
    };
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        return Some(match fs::read_link(path) {
            Ok(target) => Content::Symlink {
                points_to: link_target(path),
                target,
            },
            Err(e) => Content::Unreadable(e.to_string()),
        });
    }

    if file_type.is_dir() {
        return Some(match Directory::read(Arc::from(path)) {
            Ok(directory) => Content::Directory(directory),
            Err(e) => Content::Unreadable(e.to_string()),
        });
    }

    // Everything that is neither a directory nor a regular file is turned away
    // here, which is the one place it can be done safely.
    if !file_type.is_file() {
        return Some(Content::Refused(Refused::NotAFile));
    }

    let bytes = match read_prefix(path, limits.max_bytes) {
        Ok(bytes) => bytes,
        Err(e) => return Some(Content::Unreadable(e.to_string())),
    };
    // Reading the front of a file is cheap; making sense of it is not, and by
    // now the cursor may already be somewhere else.
    if live.cancelled() {
        return None;
    }

    // The first bytes say what the file is; the extension is not consulted,
    // because it lies and `ui::icon` already shows how many of them there are.
    // An image is then read again, whole, since it cannot be decoded from its
    // front alone.
    if let Some(format) = sniff(&bytes)
        && metadata.len() <= limits.max_image_bytes
    {
        match decode(path, format, limits, live) {
            Decoded::Image(bitmap) => return Some(Content::Image(bitmap)),
            Decoded::Cancelled => return None,
            // A file whose first bytes claim a format it does not keep is
            // shown as the bytes it actually holds.
            Decoded::Failed => (),
        }
    }

    let clipped = metadata.len() > bytes.len() as u64;
    Some(classify(bytes, clipped, limits))
}

/// The format the first bytes of a file announce, or `None` for anything that
/// is not an image this can draw.
fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    const JPEG: &[u8] = &[0xff, 0xd8, 0xff];

    if bytes.starts_with(PNG) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(JPEG) {
        Some(ImageFormat::Jpeg)
    } else {
        None
    }
}

/// What came of trying to read a file as an image.
enum Decoded {
    Image(Bitmap),
    /// Replaced while it was being decoded, which is the longest step there is.
    Cancelled,
    /// Not this format after all, or too broken to read.
    Failed,
}

/// Decodes `path` and cuts the result down to something a panel can use.
///
/// The source is bounded before anything is allocated, so an image claiming
/// impossible dimensions is refused rather than decoded; the result is scaled
/// down here rather than at drawing time, because a full sized photograph is
/// tens of megabytes to carry for a picture tens of cells across.
fn decode(path: &Path, format: ImageFormat, limits: &Limits, live: &Live<'_>) -> Decoded {
    let Ok(file) = File::open(path) else {
        return Decoded::Failed;
    };

    let mut reader = ImageReader::new(BufReader::new(file));
    reader.set_format(format);
    reader.limits(source_limits(limits.max_source_side));

    let Ok(image) = reader.decode() else {
        return Decoded::Failed;
    };
    if live.cancelled() {
        return Decoded::Cancelled;
    }

    let side = limits.max_bitmap_side;
    // `thumbnail` keeps the shape of the picture and fits it inside the
    // square, so only the longer side ever reaches the cap. Every source pixel
    // lands in exactly one of the result's, which is an area average.
    let image = if image.width() > side || image.height() > side {
        image.thumbnail(side, side)
    } else {
        image
    };

    let rgba = image.into_rgba8();
    Decoded::Image(Bitmap {
        width: rgba.width(),
        height: rgba.height(),
        pixels: rgba.into_raw(),
    })
}

/// Bounds the decoder before it allocates anything. `image::Limits` is
/// non-exhaustive, so it is built by assignment rather than as a literal.
fn source_limits(max_side: u32) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    limits
}

/// What the link resolves to, following it as far as the filesystem will.
fn link_target(path: &Path) -> LinkTarget {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => LinkTarget::Directory,
        Ok(metadata) if metadata.is_file() => LinkTarget::File,
        Ok(_) => LinkTarget::Other,
        Err(_) => LinkTarget::Broken,
    }
}

/// Whether bytes are text. A NUL in them means binary; anything else is text,
/// however little of it is valid UTF-8.
///
/// Shared with the listing, so that what the preview draws as text and what
/// entering an entry opens in an editor cannot come apart.
pub fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0)
}

/// Reads at most `max_bytes` from the front of the file. A preview of a 4 GB
/// log must not read 4 GB.
pub fn read_prefix(path: &Path, max_bytes: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(max_bytes as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Shapes bytes that are not an image. A NUL byte in the prefix means binary;
/// anything else is drawn as text, however little of it is valid UTF-8.
fn classify(bytes: Vec<u8>, clipped: bool, limits: &Limits) -> Content {
    if !is_text(&bytes) {
        return Content::Binary { bytes, clipped };
    }

    // A prefix can end in the middle of a multi-byte character. Trimming it
    // costs one character the file goes on past anyway, and keeps a stray
    // replacement glyph off the last line.
    let text = if clipped {
        &bytes[..whole_chars(&bytes)]
    } else {
        &bytes[..]
    };

    let mut lines: Vec<String> = String::from_utf8_lossy(text)
        .lines()
        .take(limits.max_lines)
        .map(|line| printable(line, limits.max_line_chars))
        .collect();
    // A file that ends without a newline still has that last line; one that is
    // clipped mid-line has a line the file does not end at.
    let more = clipped || lines.len() == limits.max_lines;
    if more {
        lines.pop();
    }

    Content::Text {
        lines,
        clipped: more,
    }
}

/// The length of the longest prefix of `bytes` that does not end part way
/// through a UTF-8 character. Only the tail is examined: an invalid byte
/// further in is a byte of a file that is not UTF-8, which is drawn lossily
/// rather than cut.
fn whole_chars(bytes: &[u8]) -> usize {
    // A character is at most four bytes, so a cut can only be in the last
    // three.
    for back in 1..=3.min(bytes.len()) {
        let index = bytes.len() - back;
        let byte = bytes[index];
        // A continuation byte says the character started earlier still.
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue;
        }
        let width = match byte {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            _ => 4,
        };
        return if back < width { index } else { bytes.len() };
    }

    bytes.len()
}

/// Turns one line into something a terminal can draw: tabs become spaces, the
/// remaining control characters are dropped, and the line is cut to `max_chars`.
fn printable(line: &str, max_chars: usize) -> String {
    let mut out = String::new();

    for c in line.chars() {
        if out.chars().count() >= max_chars {
            break;
        }
        match c {
            '\t' => out.push_str("    "),
            c if c.is_control() => (),
            c => out.push(c),
        }
    }

    out
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    use std::sync::atomic::AtomicU64;

    use crate::fs::{reader::Live, temp_tree::TempTree};

    fn limits() -> Limits {
        Limits::default()
    }

    /// Reads a path the way the job does, with nothing cancelled.
    fn read_at(path: &Path, limits: &Limits) -> Content {
        let latest = AtomicU64::new(0);
        read(path, limits, &Live::at(&latest, 0)).expect("nothing was cancelled")
    }

    #[test]
    fn a_text_file_comes_back_as_lines() {
        let tree = TempTree::new();
        let path = tree.make_file("notes.txt", "first\nsecond\n");

        let Content::Text { lines, clipped } = read_at(&path, &limits()) else {
            panic!("a text file did not read as text");
        };
        assert_eq!(lines, ["first", "second"]);
        assert!(!clipped);
    }

    #[test]
    fn a_nul_byte_makes_it_binary() {
        let tree = TempTree::new();
        // Otherwise plain ASCII, so only the NUL can be what decides.
        let path = tree.make_file("data.bin", "MZ\u{0}\u{0}program");

        assert!(matches!(read_at(&path, &limits()), Content::Binary { .. }));
    }

    #[test]
    fn the_extension_does_not_decide() {
        let tree = TempTree::new();
        let path = tree.make_file("archive.zip", "this is really just text\n");

        assert!(matches!(read_at(&path, &limits()), Content::Text { .. }));
    }

    #[test]
    fn only_the_front_of_a_long_file_is_read() {
        let tree = TempTree::new();
        let line = "x".repeat(10_000) + "\n";
        let path = tree.make_file("big.txt", &line.repeat(20));

        let capped = Limits {
            max_bytes: 1024,
            ..limits()
        };
        let Content::Text { lines, clipped } = read_at(&path, &capped) else {
            panic!("a long text file did not read as text");
        };
        assert!(clipped);
        // The line the read stopped in the middle of is dropped, so nothing is
        // shown that the file does not actually say.
        assert!(lines.is_empty(), "kept {lines:?}");
    }

    #[test]
    fn a_line_is_cut_to_the_character_cap() {
        let tree = TempTree::new();
        let path = tree.make_file("wide.txt", &format!("{}\nend\n", "ab".repeat(100)));

        let narrow = Limits {
            max_line_chars: 10,
            ..limits()
        };
        let Content::Text { lines, .. } = read_at(&path, &narrow) else {
            panic!("a wide text file did not read as text");
        };
        assert_eq!(lines[0], "ababababab");
    }

    #[test]
    fn a_directory_comes_back_as_its_listing() {
        let tree = TempTree::of(["dir/one.txt", "dir/two.txt"]);

        let Content::Directory(listing) = read_at(&tree.at("dir"), &limits()) else {
            panic!("a directory did not read as a listing");
        };
        // The parent leads the listing; the widget is what leaves it out.
        assert_eq!(listing.len(), 3);
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_is_shown_by_where_it_points() {
        let tree = TempTree::of(["real.txt"]);
        let link = tree.at("link.txt");
        std::os::unix::fs::symlink(tree.at("real.txt"), &link).unwrap();

        let Content::Symlink { target, points_to } = read_at(&link, &limits()) else {
            panic!("a symlink did not read as a symlink");
        };
        assert_eq!(points_to, LinkTarget::File);
        assert!(target.ends_with("real.txt"), "pointed at {target:?}");
    }

    #[test]
    #[cfg(unix)]
    fn a_broken_symlink_still_reports_its_target() {
        let tree = TempTree::new();
        let link = tree.at("dangling.txt");
        std::os::unix::fs::symlink(tree.at("nowhere.txt"), &link).unwrap();

        let Content::Symlink { points_to, .. } = read_at(&link, &limits()) else {
            panic!("a broken symlink did not read as a symlink");
        };
        assert_eq!(points_to, LinkTarget::Broken);
    }

    #[test]
    #[cfg(unix)]
    fn a_fifo_is_turned_away_without_being_opened() {
        let tree = TempTree::new();
        let fifo = tree.at("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo can be run");
        assert!(made.success());

        // Nothing will ever write to it, so anything that opened it would hang
        // here rather than fail.
        assert!(matches!(
            read_at(&fifo, &limits()),
            Content::Refused(Refused::NotAFile)
        ));
    }

    #[test]
    fn a_file_that_cannot_be_read_is_content_and_not_an_error() {
        let tree = TempTree::new();

        assert!(matches!(
            read_at(&tree.at("missing.txt"), &limits()),
            Content::Unreadable(_)
        ));
    }

    #[test]
    fn a_read_that_was_replaced_answers_with_nothing() {
        let tree = TempTree::new();
        let path = tree.make_file("notes.txt", "first\n");

        // The counter has moved on, which is what a newer request leaves
        // behind for a job that is already running.
        let latest = AtomicU64::new(2);
        assert!(read(&path, &limits(), &Live::at(&latest, 1)).is_none());
    }

    #[test]
    fn a_character_cut_in_half_by_the_cap_is_dropped() {
        // "č" is two bytes; cutting between them would otherwise show as a
        // replacement glyph.
        let text = "ahoj č".as_bytes();
        assert_eq!(whole_chars(&text[..text.len() - 1]), text.len() - 2);
        assert_eq!(whole_chars(text), text.len());
    }

    /// Writes a solid image of `width` by `height` into the tree.
    fn make_image(
        tree: &TempTree,
        name: &str,
        width: u32,
        height: u32,
        format: image::ImageFormat,
    ) -> PathBuf {
        let buffer = image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]));
        let path = tree.at(name);
        image::DynamicImage::from(buffer)
            .save_with_format(&path, format)
            .expect("the tree can be written to");
        path
    }

    #[test]
    fn a_png_comes_back_decoded() {
        let tree = TempTree::new();
        let path = make_image(&tree, "red.png", 4, 2, image::ImageFormat::Png);

        let Content::Image(bitmap) = read_at(&path, &limits()) else {
            panic!("a png did not read as an image");
        };
        assert_eq!((bitmap.width, bitmap.height), (4, 2));
        assert_eq!(bitmap.pixel(0, 0), [200, 30, 30, 255]);
    }

    #[test]
    fn a_jpeg_comes_back_decoded() {
        let tree = TempTree::new();
        let path = make_image(&tree, "red.jpg", 8, 4, image::ImageFormat::Jpeg);

        let Content::Image(bitmap) = read_at(&path, &limits()) else {
            panic!("a jpeg did not read as an image");
        };
        assert_eq!((bitmap.width, bitmap.height), (8, 4));
    }

    #[test]
    fn the_name_does_not_decide_that_something_is_an_image() {
        let tree = TempTree::new();
        // Named like a picture, and the bytes say otherwise.
        let path = tree.make_file("photo.png", "just text\n");

        assert!(matches!(read_at(&path, &limits()), Content::Text { .. }));
    }

    #[test]
    fn a_file_that_only_claims_to_be_a_png_falls_back_to_its_bytes() {
        let tree = TempTree::new();
        let path = tree.make_file("broken.png", "\u{89}PNG\r\n\u{1a}\n\u{0}not a png");

        // The signature got it as far as the decoder, which refused it; the
        // panel shows what is really there rather than an error.
        assert!(matches!(read_at(&path, &limits()), Content::Binary { .. }));
    }

    #[test]
    fn a_large_image_is_cut_down_before_it_is_carried_back() {
        let tree = TempTree::new();
        let path = make_image(&tree, "wide.png", 40, 20, image::ImageFormat::Png);

        let small = Limits {
            max_bitmap_side: 10,
            ..limits()
        };
        let Content::Image(bitmap) = read_at(&path, &small) else {
            panic!("a png did not read as an image");
        };
        // Scaled to fit the cap with the shape of the picture kept.
        assert_eq!((bitmap.width, bitmap.height), (10, 5));
    }

    #[test]
    fn an_image_too_large_to_read_is_shown_as_its_bytes() {
        let tree = TempTree::new();
        let path = make_image(&tree, "big.png", 64, 64, image::ImageFormat::Png);

        let tiny = Limits {
            max_image_bytes: 8,
            ..limits()
        };
        assert!(matches!(read_at(&path, &tiny), Content::Binary { .. }));
    }

    #[test]
    fn tabs_become_spaces_and_other_control_characters_go_away() {
        assert_eq!(printable("a\tb\u{7}c", 100), "a    bc");
    }
}
