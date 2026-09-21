//! Packing a batch of items into an archive, and unpacking one back out.
//!
//! Both are jobs like any other: the paths are a snapshot taken when the job
//! was queued, every entry is reported to an [`Observer`](crate::fs::ops::Observer)
//! and asked about before it is written, and what could not be done comes
//! back as a summary rather than as an error.
//!
//! Unpacking writes into a staging directory beside the destination and gives
//! it its final name with one `rename` at the end, so an unpack is all of an
//! archive or none of it.

pub(crate) mod contents;
pub(crate) mod format;
pub(crate) mod pack;
pub(crate) mod unpack;
