use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style, Stylize},
    symbols,
    text::{Line, Span},
    widgets::Widget,
};

use crate::{keys::KeyBinding, ui};

/// What the caller has already worked out about the running job. The widget
/// only lays it out; the ratio is computed where the counters live.
#[derive(Clone, Copy, Debug)]
pub struct ProgressView<'a> {
    pub ratio: f64,
    pub current: &'a str,
    pub done: usize,
    pub total: usize,
}

/// The row above the key bar. Draws the state of the focused tab and of the
/// background worker as segments separated by `SEPARATOR`.
///
/// Segments are laid out in the order `segments` returns them, and the first
/// one that does not fit in `area` ends the row — it and everything after it
/// are left out. So the order is also the priority: what a stalled copy is
/// doing matters more than how many items are marked.
#[derive(Debug, Default)]
pub struct InfoBar<'a> {
    marked: usize,
    queued: usize,
    progress: Option<ProgressView<'a>>,
    /// Drawn only while a job runs. Cancelling is the one key whose meaning is
    /// transient, so it is advertised here rather than holding a slot in the
    /// key bar for the whole session. Resolved by the caller from the key
    /// table, so the two cannot drift apart.
    cancel_key: Option<KeyBinding>,
    /// What the focused pane is filtered to, or `None` when it is not.
    ///
    /// Unlike the dot files, this is not on the screen anywhere: a filtered
    /// listing looks exactly like a short directory. `Some("")` is the moment
    /// after the filter key, with nothing typed yet.
    filter: Option<&'a str>,
    tick: u64,
}

impl<'a> InfoBar<'a> {
    /// Drawn between two segments, never before the first or after the last.
    const SEPARATOR: &'static str = " | ";

    /// Width of the progress bar in cells.
    const BAR_WIDTH: usize = 16;

    /// Longest a file name may be drawn before it is clipped, so a deep name
    /// cannot push the counts off the row.
    const NAME_WIDTH: usize = 24;

    /// Follows the count of marked items.
    const MARKED_LABEL: &'static str = " marked";

    /// Follows the count of jobs waiting behind the running one.
    const QUEUED_LABEL: &'static str = " queued";

    /// Follows the cancel key, which is drawn from the key table.
    const CANCEL_LABEL: &'static str = " cancel";

    /// Precedes what the listing is filtered to.
    const FILTER_LABEL: &'static str = "filter: ";

    /// Longest a filter may be drawn before it is clipped.
    const FILTER_WIDTH: usize = 20;

    /// One frame per tick of the main loop. It turns whether or not the bar
    /// moves, which is the whole point: a single huge file leaves the bar
    /// still, and only this says the copy is alive rather than wedged.
    const SPINNER: [&'static str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

    pub fn new() -> Self {
        Self::default()
    }

    pub fn marked(mut self, count: usize) -> Self {
        self.marked = count;
        self
    }

    pub fn queued(mut self, count: usize) -> Self {
        self.queued = count;
        self
    }

    pub fn progress(mut self, progress: Option<ProgressView<'a>>) -> Self {
        self.progress = progress;
        self
    }

    pub fn cancel_key(mut self, key: Option<KeyBinding>) -> Self {
        self.cancel_key = key;
        self
    }

    pub fn filter(mut self, filter: Option<&'a str>) -> Self {
        self.filter = filter;
        self
    }

    pub fn tick(mut self, tick: u64) -> Self {
        self.tick = tick;
        self
    }

    /// Returns the segments in the order they are drawn, most important first.
    /// A segment with nothing to report is omitted, so an idle bar is empty.
    fn segments(&self) -> Vec<Vec<Span<'a>>> {
        let mut segments = Vec::new();

        if let Some(progress) = self.progress {
            let (done, rest) = Self::bar(progress.ratio);
            segments.push(vec![
                Span::raw(Self::SPINNER[(self.tick as usize) % Self::SPINNER.len()])
                    .fg(Color::Cyan),
                Span::raw(" "),
                Span::raw(done).fg(Color::Cyan),
                Span::raw(rest).fg(Color::DarkGray),
                Span::raw(format!(" {:>3.0}%", progress.ratio * 100.0)).bold(),
            ]);

            if !progress.current.is_empty() {
                segments.push(vec![
                    Span::raw(ui::clip(progress.current, Self::NAME_WIDTH)).fg(Color::Gray),
                ]);
            }

            segments.push(vec![
                Span::raw(format!("{} / {}", progress.done, progress.total)).bold(),
            ]);
        }

        // Ahead of the marks: a filter changes what the next operation will
        // act on, and a reader who has forgotten it is filtering is exactly
        // who the segment is for.
        if let Some(filter) = self.filter {
            segments.push(vec![
                Span::raw(Self::FILTER_LABEL).fg(Color::Gray),
                Span::raw(ui::clip(filter, Self::FILTER_WIDTH)).bold(),
            ]);
        }

        if self.marked > 0 {
            segments.push(vec![
                Span::raw(format!("{}{}", self.marked, Self::MARKED_LABEL)).bold(),
            ]);
        }

        // Counts the running job too, so the row never reads as empty while
        // something is still on the queue.
        if self.queued > 1 {
            segments.push(vec![
                Span::raw(format!("{}{}", self.queued - 1, Self::QUEUED_LABEL)).fg(Color::Gray),
            ]);
        }

        // Last, so a row too narrow for everything drops the hint before it
        // drops live numbers: the key is learnt once, the counts are not.
        if let (Some(progress), Some(key)) = (self.progress, self.cancel_key)
            && progress.ratio < 1.0
        {
            // `to_span` would borrow the local key; the whole binding is
            // formatted so the modifier prefix comes along with the code.
            segments.push(vec![
                Span::raw(key.to_string()).bold(),
                Span::raw(Self::CANCEL_LABEL).fg(Color::Gray),
            ]);
        }

        segments
    }

    /// Splits the bar into its filled and unfilled halves. The filled half ends
    /// in a partial block, so the bar moves in eighths of a cell instead of
    /// jumping a whole one at a time.
    fn bar(ratio: f64) -> (String, String) {
        const EIGHTHS: [&str; 8] = [
            "",
            symbols::block::ONE_EIGHTH,
            symbols::block::ONE_QUARTER,
            symbols::block::THREE_EIGHTHS,
            symbols::block::HALF,
            symbols::block::FIVE_EIGHTHS,
            symbols::block::THREE_QUARTERS,
            symbols::block::SEVEN_EIGHTHS,
        ];

        let filled = ratio.clamp(0.0, 1.0) * Self::BAR_WIDTH as f64;
        let whole = filled.trunc() as usize;
        // `fract` stays below one cell, so the index cannot run off the table.
        let partial = EIGHTHS[((filled.fract() * 8.0) as usize).min(EIGHTHS.len() - 1)];

        let mut done = symbols::block::FULL.repeat(whole);
        done.push_str(partial);

        let drawn = whole + usize::from(!partial.is_empty());
        (done, symbols::shade::LIGHT.repeat(Self::BAR_WIDTH - drawn))
    }
}

impl Widget for InfoBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Sets the foreground only; the row keeps the terminal background.
        buf.set_style(area, Style::new().fg(Color::White));

        let mut spans: Vec<Span> = Vec::new();
        let mut width = 0;
        for segment in self.segments() {
            let separator = if spans.is_empty() {
                0
            } else {
                Span::raw(Self::SEPARATOR).width()
            };
            let segment_width: usize = segment.iter().map(Span::width).sum();
            if width + separator + segment_width > area.width as usize {
                break;
            }

            width += separator + segment_width;
            if separator > 0 {
                spans.push(Span::raw(Self::SEPARATOR));
            }
            spans.extend(segment);
        }

        Line::from(spans).render(area, buf);
    }
}

#[cfg(test)]
mod infobar_tests {
    use super::*;

    /// Renders the bar into a one-row buffer of the given width and returns the
    /// symbols of that row as a single string.
    fn render(bar: InfoBar, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
        (0..width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn halfway() -> ProgressView<'static> {
        ProgressView {
            ratio: 0.5,
            current: "file.txt",
            done: 6,
            total: 12,
        }
    }

    #[test]
    fn the_marked_count_is_drawn() {
        assert_eq!(render(InfoBar::new().marked(3), 40), marked(3));
    }

    #[test]
    fn nothing_marked_leaves_the_row_empty() {
        assert_eq!(render(InfoBar::new(), 40), "");
    }

    #[test]
    fn a_segment_that_does_not_fit_is_dropped() {
        assert_eq!(render(InfoBar::new().marked(12), 5), "");
    }

    #[test]
    fn progress_draws_a_half_filled_bar_with_its_counts() {
        assert_eq!(
            render(InfoBar::new().progress(Some(halfway())), 80),
            "⠋ ████████░░░░░░░░  50% | file.txt | 6 / 12"
        );
    }

    #[test]
    fn progress_comes_before_the_marked_count() {
        let row = render(InfoBar::new().progress(Some(halfway())).marked(3), 80);
        assert!(row.ends_with(&format!("6 / 12 | {}", marked(3))), "{row}");
    }

    #[test]
    fn a_narrow_row_keeps_the_bar_and_drops_the_rest() {
        assert_eq!(
            render(InfoBar::new().progress(Some(halfway())).marked(3), 26),
            "⠋ ████████░░░░░░░░  50%"
        );
    }

    #[test]
    fn the_spinner_turns_with_the_tick() {
        let first = render(InfoBar::new().progress(Some(halfway())).tick(0), 30);
        let second = render(InfoBar::new().progress(Some(halfway())).tick(1), 30);
        assert_ne!(first, second);
    }

    #[test]
    fn the_bar_fills_in_eighths_of_a_cell() {
        // One sixteenth of a sixteen-cell bar is exactly half of the first cell.
        let (done, rest) = InfoBar::bar(1.0 / 32.0);
        assert_eq!(done, "▌");
        assert_eq!(rest.chars().count(), 15);
    }

    #[test]
    fn a_full_bar_leaves_nothing_unfilled() {
        let (done, rest) = InfoBar::bar(1.0);
        assert_eq!(done.chars().count(), InfoBar::BAR_WIDTH);
        assert_eq!(rest, "");
    }

    /// The widget draws whatever key it is handed, so which one the table
    /// actually binds is `keys.rs`'s business, not this module's.
    fn cancel_key() -> Option<KeyBinding> {
        Some(KeyBinding::plain(ratatui::crossterm::event::KeyCode::Char(
            'x',
        )))
    }

    /// The hint as `segments` assembles it: the key, then the label. The key
    /// and the label are drawn as one unit, so this is what both presence and
    /// absence are asserted against — a one-character key on its own turns up
    /// inside a file name.
    fn cancel_hint() -> String {
        format!("{}{}", cancel_key().unwrap(), InfoBar::CANCEL_LABEL)
    }

    /// A count as `segments` draws it, taking the label from the widget.
    fn marked(count: usize) -> String {
        format!("{count}{}", InfoBar::MARKED_LABEL)
    }

    #[test]
    fn the_cancel_hint_shows_only_while_a_job_runs() {
        let key = cancel_key();

        let running = render(InfoBar::new().progress(Some(halfway())).cancel_key(key), 80);
        assert!(running.ends_with(&cancel_hint()), "{running}");

        // Nothing queued, so there is nothing the key would do.
        assert_eq!(render(InfoBar::new().cancel_key(key), 80), "");
    }

    #[test]
    fn a_finished_bar_drops_the_cancel_hint() {
        let done = ProgressView {
            ratio: 1.0,
            ..halfway()
        };

        let row = render(
            InfoBar::new().progress(Some(done)).cancel_key(cancel_key()),
            80,
        );
        assert!(!row.contains(&cancel_hint()), "{row}");
    }

    #[test]
    fn the_hint_is_the_first_thing_a_narrow_row_drops() {
        let key = cancel_key();
        // Wide enough for the counts, too narrow for the hint behind them.
        let row = render(
            InfoBar::new()
                .progress(Some(halfway()))
                .marked(3)
                .cancel_key(key),
            60,
        );

        assert!(row.contains(&marked(3)), "{row}");
        assert!(!row.contains(&cancel_hint()), "{row}");
    }

    #[test]
    fn the_queue_counts_only_what_is_waiting() {
        assert_eq!(render(InfoBar::new().queued(1), 40), "");
        assert_eq!(
            render(InfoBar::new().queued(3), 40),
            format!("2{}", InfoBar::QUEUED_LABEL)
        );
    }
}
