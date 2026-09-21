use std::path::{Path, PathBuf};

use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Padding, StatefulWidget, Widget},
};

use crate::{
    action::VerticalDir,
    keys::{Binding, KeyBinding},
    ui,
};

/// `wanted` brought inside `min`..`max` hundredths of `whole`. A band too
/// narrow to hold what was asked for is what makes the list scroll.
fn banded(wanted: u16, whole: u16, min: u16, max: u16) -> u16 {
    let share = |percent: u16| (whole as u32 * percent as u32 / 100) as u16;
    wanted.clamp(share(min), share(max))
}

/// The last [`FavoritesView::TAIL`] components of `path`, with an ellipsis
/// standing in for the rest: `…/Rust/mula/docs`. A path with no more
/// components than that is given whole.
///
/// The end rather than the start, and cut here rather than by
/// [`ui::clip`]: what tells two favorites apart is where they end, and the
/// directories they are in are the part they have in common.
fn tail(path: &Path) -> String {
    let components: Vec<_> = path.components().collect();
    let Some(kept) = components.len().checked_sub(FavoritesView::TAIL) else {
        return path.to_string_lossy().into_owned();
    };
    if kept == 0 {
        return path.to_string_lossy().into_owned();
    }

    let tail: PathBuf = components[kept..].iter().collect();
    format!("…/{}", tail.display())
}

#[derive(Clone, Copy, Debug)]
pub enum FavoritesMsg {
    MoveSelection(VerticalDir),
    /// Goes to the directory under the cursor.
    Confirm,
    /// Takes the directory under the cursor off the list. Nothing on disk is
    /// touched, so nothing is asked first.
    Remove,
    Cancel,
}

/// The favorites overlay: a cursor over a list the overlay does not own.
///
/// The list itself lives on `App`, since it outlives every time the overlay is
/// opened and is written to a file of its own. What is here is what belongs to
/// the overlay alone — where the cursor stands while it is up.
#[derive(Debug)]
pub struct FavoritesView {
    list_state: ListState,
}

impl FavoritesView {
    pub const FAVORITE_KEYS: &[Binding<FavoritesMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Up),
            msg: FavoritesMsg::MoveSelection(VerticalDir::Up),
            bar: None,
            help: "Moves the cursor one favorite up",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Down),
            msg: FavoritesMsg::MoveSelection(VerticalDir::Down),
            bar: None,
            help: "Moves the cursor one favorite down",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: FavoritesMsg::Confirm,
            bar: Some("Go to"),
            help: "Opens the directory under the cursor in this panel",
        },
        // `d` carries the label and `Delete` answers beside it: a Mac laptop
        // has no key of its own for `Delete`, so a bar naming it would be
        // telling half its readers to press `fn+Backspace`.
        Binding {
            key: KeyBinding::plain(KeyCode::Char('d')),
            msg: FavoritesMsg::Remove,
            bar: Some("Remove"),
            help: "Takes the directory under the cursor off the list",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Delete),
            msg: FavoritesMsg::Remove,
            bar: None,
            help: "Takes the directory under the cursor off the list",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: FavoritesMsg::Cancel,
            bar: Some("Cancel"),
            help: "Closes the overlay, staying where the panel is",
        },
    ];

    const CURSOR_BG: Color = Color::Rgb(0x33, 0x3c, 0x4d);

    /// The cursor's own column to the left of every row, held open whether the
    /// row is the one or not.
    const HIGHLIGHT: &'static str = "> ";

    const TITLE: &'static str = "Favorites";

    /// The band the box is allowed to take of the terminal, in hundredths of
    /// it. Inside the band it is sized to what it draws; a floor keeps a list
    /// of two from being a box too small to read as one, and a ceiling keeps
    /// a list of eighty from covering the panels it was opened over.
    ///
    /// A share of the terminal rather than a number of cells: how wide the
    /// reader's terminal is, is not ours to guess, and what is cut off by the
    /// ceiling scrolls.
    const MIN_WIDTH_PERCENT: u16 = 30;
    const MAX_WIDTH_PERCENT: u16 = 80;
    const MIN_HEIGHT_PERCENT: u16 = 25;
    const MAX_HEIGHT_PERCENT: u16 = 80;

    /// How much of a path is kept when the whole of it does not fit. The end
    /// of it: a favorite is told apart from the others by where it ends, and
    /// every one of them starts in the same home directory.
    const TAIL: usize = 3;

    /// Opens with the cursor on the first favorite, and on nothing at all
    /// while there are none.
    pub fn new(count: usize) -> Self {
        let mut list_state = ListState::default();
        if count > 0 {
            list_state.select(Some(0));
        }
        Self { list_state }
    }

    pub fn selected(&self) -> Option<usize> {
        self.list_state.selected()
    }

    /// Moves the cursor through the favorites, wrapping at either end. Does
    /// nothing while there are none.
    pub fn move_selection(&mut self, dir: VerticalDir, count: usize) {
        let Some(index) = self.list_state.selected() else {
            return;
        };
        if count == 0 {
            return;
        }

        let next = match dir {
            VerticalDir::Up => (index + count - 1) % count,
            VerticalDir::Down => (index + 1) % count,
        };
        self.list_state.select(Some(next));
    }

    /// Brings the cursor back into a list that has just lost an entry: it
    /// stays on the row it was on, which is now the entry below the one that
    /// went away, and steps back when that row was the last.
    pub fn fit(&mut self, count: usize) {
        match count {
            0 => self.list_state.select(None),
            count => {
                let index = self.list_state.selected().unwrap_or(0);
                self.list_state.select(Some(index.min(count - 1)));
            }
        }
    }

    fn block() -> Block<'static> {
        Block::bordered()
            .title_top(Line::from(Self::TITLE).centered())
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(Color::Black).fg(Color::Blue))
    }

    /// The box, centred, sized to what it draws and kept inside its band of
    /// the terminal.
    ///
    /// Sized to the list rather than to a share of the screen: three
    /// favorites in a box covering half the terminal say nothing about what is
    /// in them. The chrome comes from the `Block` itself, so the padding and
    /// the borders cannot be counted wrong here.
    fn outer_area(&self, paths: &[PathBuf], add_key: Option<KeyBinding>, area: Rect) -> Rect {
        let widest = paths
            .iter()
            .map(|path| Span::raw(path.to_string_lossy()).width() + Self::highlight_width())
            .chain([
                self.widest_status(paths, add_key),
                Span::raw(Self::TITLE).width(),
            ])
            .max()
            .unwrap_or(0);

        // What the block takes for its borders and padding, measured off an
        // area it is given rather than counted.
        let probe = Rect::new(0, 0, area.width, area.height);
        let inner = Self::block().inner(probe);
        let chrome_x = probe.width - inner.width;
        let chrome_y = probe.height - inner.height;

        // The list, the status line under it, and the chrome around both.
        let wanted_width = u16::try_from(widest)
            .unwrap_or(u16::MAX)
            .saturating_add(chrome_x);
        let wanted_height = u16::try_from(paths.len())
            .unwrap_or(u16::MAX)
            .saturating_add(1)
            .saturating_add(chrome_y);

        let width = banded(
            wanted_width,
            area.width,
            Self::MIN_WIDTH_PERCENT,
            Self::MAX_WIDTH_PERCENT,
        );
        let height = banded(
            wanted_height,
            area.height,
            Self::MIN_HEIGHT_PERCENT,
            Self::MAX_HEIGHT_PERCENT,
        );

        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);
        area
    }

    /// Splits the inside of the box into the list and the status line under it.
    fn rows(outer: Rect) -> [Rect; 2] {
        let inner = Self::block().inner(outer);
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner)
    }

    fn highlight_width() -> usize {
        Span::raw(Self::HIGHLIGHT).width()
    }

    /// What the line under the list says: which rows of a long list are on
    /// screen, or how to put something on an empty one.
    fn status(&self, paths: &[PathBuf], add_key: Option<KeyBinding>, rows: usize) -> String {
        if paths.is_empty() {
            return Self::hint(add_key);
        }

        let first = self.list_state.offset().min(paths.len().saturating_sub(1));
        let shown = rows.min(paths.len() - first);
        ui::scroll_position(first, shown, paths.len()).unwrap_or_default()
    }

    fn hint(add_key: Option<KeyBinding>) -> String {
        match add_key {
            Some(key) => format!("Nothing here yet — {key} adds the directory you are in"),
            None => String::from("Nothing here yet"),
        }
    }

    /// The widest the status line can get, which the box has to be wide
    /// enough for before it is known which rows are on screen.
    ///
    /// Every number in it is a row of this list, so the widest it gets is the
    /// last row counted twice. A list short enough to be drawn whole has no
    /// status line and asks for no width at all.
    fn widest_status(&self, paths: &[PathBuf], add_key: Option<KeyBinding>) -> usize {
        let text = match paths.len() {
            0 => Self::hint(add_key),
            len => ui::scroll_position(len - 1, 1, len).unwrap_or_default(),
        };
        Span::raw(text).width()
    }

    /// Draws the box over `area`, listing `paths`.
    ///
    /// The paths are handed in rather than held: the list belongs to `App`,
    /// and a widget that reached for it would be reading the state it draws.
    pub fn render(
        &mut self,
        paths: &[PathBuf],
        add_key: Option<KeyBinding>,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let outer = self.outer_area(paths, add_key, area);
        Clear.render(outer, buf);
        Self::block().render(outer, buf);

        let [list_area, status_area] = Self::rows(outer);

        let room = (list_area.width as usize).saturating_sub(Self::highlight_width());
        let items: Vec<ListItem> = paths
            .iter()
            .map(|path| {
                // The whole path while it fits, its last components when the
                // box has been stopped by the terminal before it did.
                let full = path.to_string_lossy();
                let text = match Span::raw(&*full).width() <= room {
                    true => full.into_owned(),
                    false => ui::clip(&tail(path), room),
                };
                ListItem::new(Line::from(Span::styled(
                    text,
                    Style::new().fg(Color::White),
                )))
            })
            .collect();

        StatefulWidget::render(
            List::new(items)
                .highlight_style(Style::new().bg(Self::CURSOR_BG))
                .highlight_symbol(Self::HIGHLIGHT),
            list_area,
            buf,
            &mut self.list_state,
        );

        Line::from(Span::raw(self.status(paths, add_key, list_area.height as usize)).dim())
            .render(status_area, buf);
    }
}

#[cfg(test)]
mod favorites_view_tests {
    use super::*;

    /// The smallest terminal that counts.
    const SMALL: Rect = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };

    fn paths(count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|i| PathBuf::from(format!("/home/user/place-{i}")))
            .collect()
    }

    fn rendered(view: &mut FavoritesView, paths: &[PathBuf], area: Rect) -> Vec<String> {
        let mut buf = Buffer::empty(area);
        view.render(paths, None, area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// A short list gets a small box: every path is drawn whole and on a row
    /// of its own, and the terminal is left with room to spare.
    #[test]
    fn a_short_list_leaves_the_terminal_room_to_spare() {
        let paths = paths(3);
        let view = FavoritesView::new(paths.len());
        let widest = paths
            .iter()
            .map(|path| Span::raw(path.to_string_lossy()).width())
            .max()
            .unwrap();

        let outer = view.outer_area(&paths, None, SMALL);

        let [list, _] = FavoritesView::rows(outer);
        assert!(
            list.width as usize >= widest,
            "a path of {widest} columns is drawn into {}",
            list.width
        );
        assert!(
            list.height as usize >= paths.len(),
            "{} rows hold {} favorites",
            list.height,
            paths.len()
        );
        assert!(
            outer.width < SMALL.width && outer.height < SMALL.height,
            "the box is {}x{} on a {}x{} terminal",
            outer.width,
            outer.height,
            SMALL.width,
            SMALL.height
        );
    }

    /// Two favorites are not a box two rows tall: the floor of the band holds
    /// it open, and what holds it open is a share of the terminal.
    #[test]
    fn a_list_of_two_still_looks_like_a_list() {
        let paths = paths(2);
        let view = FavoritesView::new(paths.len());

        let small = view.outer_area(&paths, None, SMALL);
        let large = view.outer_area(&paths, None, Rect::new(0, 0, 200, 60));

        assert!(small.height > paths.len() as u16, "{small:?}");
        assert!(large.height > small.height, "{small:?} against {large:?}");
        assert!(large.width > small.width, "{small:?} against {large:?}");
    }

    /// The box stops before the terminal does, and the rest of the list
    /// scrolls inside what it took.
    #[test]
    fn a_long_list_stops_short_of_the_terminal() {
        let paths = paths(60);
        let view = FavoritesView::new(paths.len());

        let outer = view.outer_area(&paths, None, SMALL);

        assert!(outer.height < SMALL.height, "{outer:?}");
        assert!((outer.height as usize) < paths.len(), "{outer:?}");
    }

    /// A path the box cannot hold keeps the end that tells it apart from the
    /// others, rather than being cut where it runs out of room.
    #[test]
    fn a_path_too_long_for_the_box_keeps_its_last_components() {
        let deep = PathBuf::from("/home/user/Programming/Rust/mula/docs/notes/deep");
        let paths = vec![deep];
        let mut view = FavoritesView::new(paths.len());

        let rows = rendered(&mut view, &paths, Rect::new(0, 0, 40, 24));

        assert!(
            rows.iter().any(|row| row.contains("…/docs/notes/deep")),
            "the end of the path is on no row of {rows:#?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("Programming")),
            "the head of the path was drawn into {rows:#?}"
        );
    }

    #[test]
    fn a_path_with_few_enough_components_is_given_whole() {
        assert_eq!(tail(Path::new("/one/two")), "/one/two");
        assert_eq!(tail(Path::new("/a/b/c/d")), "…/b/c/d");
    }

    /// The box is sized to the terminal and the list scrolls inside it, so a
    /// list longer than the box is still a list every entry can be reached in.
    #[test]
    fn every_favorite_can_be_reached_on_an_eighty_by_twenty_four_terminal() {
        let paths = paths(60);
        let mut view = FavoritesView::new(paths.len());

        for (index, path) in paths.iter().enumerate() {
            view.list_state.select(Some(index));
            let rows = rendered(&mut view, &paths, SMALL);
            let name = path.to_string_lossy().into_owned();
            assert!(
                rows.iter().any(|row| row.contains(&name)),
                "{name} is on no row of {rows:#?}"
            );
        }
    }

    #[test]
    fn an_empty_list_says_how_to_put_something_on_it() {
        let mut view = FavoritesView::new(0);
        let key = KeyBinding::plain(KeyCode::Char('B')).shift();

        let mut buf = Buffer::empty(SMALL);
        view.render(&[], Some(key), SMALL, &mut buf);
        let rows: Vec<String> = (0..SMALL.height)
            .map(|y| {
                (0..SMALL.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();

        assert!(
            rows.iter().any(|row| row.contains("Shift+B")),
            "the key is on no row of {rows:#?}"
        );
    }

    /// The row a removal leaves behind holds the entry that was under it, so
    /// the cursor stays where the user is looking.
    #[test]
    fn the_cursor_stays_on_its_row_when_the_list_shrinks() {
        let mut view = FavoritesView::new(4);
        view.move_selection(VerticalDir::Down, 4);

        view.fit(3);

        assert_eq!(view.selected(), Some(1));
    }

    #[test]
    fn the_cursor_steps_back_off_the_end_of_a_list_that_shrank() {
        let mut view = FavoritesView::new(3);
        view.move_selection(VerticalDir::Up, 3);
        assert_eq!(view.selected(), Some(2));

        view.fit(2);

        assert_eq!(view.selected(), Some(1));
    }

    #[test]
    fn the_last_favorite_taken_off_the_list_leaves_the_cursor_on_nothing() {
        let mut view = FavoritesView::new(1);

        view.fit(0);

        assert_eq!(view.selected(), None);
    }
}
