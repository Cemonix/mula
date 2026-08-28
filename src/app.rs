use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    process::Child,
    sync::Arc,
    time::Duration,
};

use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    layout::{Constraint, Direction, Flex, Layout, Rect},
};
use thiserror::Error;

use crate::{
    action::{Action, ListEnd, VerticalDir},
    fs::{
        directory::DirEntryKind,
        find::{Found, Limits, Search},
        job::{JobKind, JobTag, Outcome, Progress, Work},
        listing::Kind,
        ops::{MutationOp, ProcessedSummary, TransferOp},
        preview::{self, Content, Preview},
        reader::Reader,
        worker::{Health, Worker},
    },
    keys::{self, GlobalMsg, KeyBinding},
    open::{self, Handover, Launch, Opener, Program},
    ui::{
        dialog::{Choice, Dialog, DialogMsg},
        finder::{FindMsg, Finder},
        graphics::{
            capabilities::Capabilities,
            surface::{Placement, Surface, Wanted},
        },
        help::{self, Help, HelpMsg, HelpState},
        infobar::{InfoBar, ProgressView},
        keybar::Keybar,
        pane::{Entered, Pane, PaneError},
        panel::Panel,
        preview::PreviewPane,
        prompt::{InputMsg, Prompt},
        tab::{MarkOp, Tab, TabId, TabList},
        toast::{Toast, ToastLevel},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn toggle(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// What the panel opposite the cursor shows.
///
/// Deliberately not a `Mode`. A mode takes over the key table; this only
/// changes what is drawn, so the browse keys and every operation under them
/// go on working while the preview is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opposite {
    Listing,
    Preview,
}

impl Opposite {
    pub fn toggle(self) -> Self {
        match self {
            Opposite::Listing => Opposite::Preview,
            Opposite::Preview => Opposite::Listing,
        }
    }
}

#[derive(Debug)]
pub enum Mode {
    Browse,
    Confirm {
        dialog: Dialog,
        pending: ConfirmTarget,
    },
    Input {
        prompt: Prompt,
        pending: InputTarget,
    },
    Find {
        finder: Finder,
    },
}

/// A filesystem mutation that still needs the name being typed.
#[derive(Debug, Clone)]
pub enum PendingMutation {
    Rename(Arc<Path>),
    CreateEntry(Arc<Path>),
}

impl PendingMutation {
    /// A name ending in `/` creates a directory; anything else creates a
    /// file. Either may carry intermediate directories that do not exist
    /// yet (`fol/fol2/fol3/file1.txt`), which the op creates along the way.
    pub fn op(self, name: String) -> MutationOp {
        match self {
            PendingMutation::Rename(target) => MutationOp::Rename {
                path: target.to_path_buf(),
                new_name: name,
            },
            PendingMutation::CreateEntry(parent) => match name.strip_suffix('/') {
                Some(dirs) => MutationOp::CreateDir {
                    parent: parent.to_path_buf(),
                    name: dirs.to_string(),
                },
                None => MutationOp::CreateFile {
                    parent: parent.to_path_buf(),
                    name,
                },
            },
        }
    }
}

/// Where the text of a confirmed `Input` prompt goes: into a filesystem
/// mutation, or into the title of the tab that was open for renaming.
#[derive(Debug, Clone)]
pub enum InputTarget {
    Mutation(PendingMutation),
    RenameTab,
}

/// What a `Confirm` dialog carries out once it is answered with `Yes`.
///
/// Its own type rather than an `Action`, the way [`InputTarget`] is: these are
/// the halves that do not ask, and nothing that resolves a key can name them.
#[derive(Debug, Clone, Copy)]
pub enum ConfirmTarget {
    Delete,
    Quit,
}

/// A file waiting to be handed the terminal, at the end of the pass of the
/// loop it was asked for in. Handing over needs the terminal, and only `run`
/// holds it.
///
/// The program rather than the opener, so a detached one cannot end up in a
/// slot that only ever hands the terminal over.
#[derive(Debug)]
struct Opening {
    program: Program,
    path: Arc<Path>,
}

/// Where the marks a queued job took came from, so the items that fail can be
/// put back even after the user has switched tabs or panels.
#[derive(Debug)]
struct QueuedMarks {
    tag: JobTag,
    side: Side,
    tab: TabId,
}

#[derive(Error, Debug)]
pub enum AppError {
    #[error(transparent)]
    Pane(#[from] PaneError),
    #[error(transparent)]
    IO(#[from] io::Error),
}

#[derive(Debug)]
pub struct App {
    left: Panel,
    right: Panel,
    focused_side: Side,
    toasts: VecDeque<Toast>,
    mode: Mode,
    /// Open over whatever mode is running, showing that mode's keys. Not a
    /// `Mode` itself: the mode underneath has to stay alive to be described.
    help: Option<HelpState>,
    help_key: Option<KeyBinding>,
    /// Resolved from the key table once, so the hint the info bar draws while a
    /// job runs cannot drift from the key that actually cancels it.
    cancel_key: Option<KeyBinding>,
    worker: Worker,
    /// Serves the find overlay. Separate from `worker` because a walk of a
    /// large tree queued behind a copy would report its first hit minutes late,
    /// and because reads need no ordering against each other.
    reader: Reader<Search>,
    /// What the panel opposite the cursor shows.
    opposite: Opposite,
    /// Serves the Quick View panel. A reader of its own rather than another
    /// job on `reader`: one generation counter for both would cancel a running
    /// walk every time the cursor moved, and would hold together only because
    /// the find overlay happens to be modal.
    previewer: Reader<Preview>,
    /// The path the outstanding preview was asked for, and so the thing that
    /// says whether a new one is needed.
    previewing: Option<Arc<Path>>,
    /// The newest answer, or `None` while one is on its way.
    preview: Option<Content>,
    /// Counts the answers the panel has been given, so a picture is told from
    /// the one it replaced even where both were read from the same path.
    preview_generation: u64,
    /// Where the panel put a picture the terminal draws itself, settled by the
    /// last frame and `None` for a panel drawing half blocks or nothing.
    preview_placement: Option<Placement>,
    /// The pictures the terminal is holding. Only this writes them, and only
    /// after a frame has been drawn.
    surface: Surface,
    /// The newest snapshot of the running job, or `None` while the queue is
    /// empty. Derived from the worker and true only while it works, so it
    /// belongs in the info bar rather than in a toast.
    progress: Option<Progress>,
    /// What the loop is to hand the terminal over for, or `None` when nothing
    /// asked.
    opening: Option<Opening>,
    /// The programs started without the terminal. Nothing waits for them; the
    /// list is only what `try_wait` is asked on, so one that has ended is let
    /// go of rather than left in the process table.
    detached: Vec<Child>,
    queued_marks: Vec<QueuedMarks>,
    /// Turns once per pass of the loop and drives the spinner. Nothing else
    /// reads it, so wrapping is harmless.
    tick: u64,
    exit: bool,
}

impl App {
    /// How long the loop waits for a key before it comes back around to expire
    /// toasts. Also the coarsest delay a toast can outlive its duration by.
    const TICK: Duration = Duration::from_millis(100);

    /// A terminal that draws its own pictures is worth reading more pixels
    /// for: a panel is tens of cells across and a cell is tens of pixels, so
    /// the ceiling is roughly what a full-screen panel could ask for.
    const GRAPHICS_BITMAP_SIDE: u32 = 1536;

    pub fn new(capabilities: Capabilities) -> Result<Self, AppError> {
        let mut preview_limits = preview::Limits::default();
        if capabilities.graphics().is_some() {
            preview_limits.max_bitmap_side = Self::GRAPHICS_BITMAP_SIDE;
        }

        Ok(Self {
            left: Panel::new()?,
            right: Panel::new()?,
            focused_side: Side::Left,
            toasts: VecDeque::new(),
            mode: Mode::Browse,
            help: None,
            help_key: keys::find(keys::GLOBAL_KEYS, |m| matches!(m, GlobalMsg::ShowHelp))
                .map(|binding| binding.key),
            cancel_key: keys::find(keys::BROWSE_KEYS, |a| matches!(a, Action::CancelJob))
                .map(|binding| binding.key),
            worker: Worker::start(),
            reader: Reader::<Search>::start(Limits::default()),
            opposite: Opposite::Listing,
            previewer: Reader::<Preview>::start(preview_limits),
            previewing: None,
            preview: None,
            preview_generation: 0,
            preview_placement: None,
            surface: Surface::new(capabilities),
            progress: None,
            opening: None,
            detached: Vec::new(),
            queued_marks: Vec::new(),
            tick: 0,
            exit: false,
        })
    }

    /// Draws, waits up to `TICK` for a key, then takes whatever the worker has
    /// sent and drops the toasts that ran out. The wait is what paces the loop:
    /// it already had to come back around to expire toasts, so the worker's
    /// channel needs no waking of its own.
    ///
    /// Errors that reach this far come from reading the terminal itself and end
    /// the loop; everything an action can fail at is turned into a toast by
    /// `handle_events`.
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<(), AppError> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            // After the draw and not inside it: a picture is laid over the
            // text, so the text has to have reached the screen first.
            self.place_preview();
            self.handle_events()?;
            self.collect_from_worker();
            self.collect_from_reader();
            // After the worker, since finishing a job asks both panes to read
            // their directories again. Sending before draining is what keeps a
            // replaced request's answer from being folded in: the send raises
            // the generation, and the drain that follows drops what is stale.
            self.sync_listings();
            self.collect_from_listings();
            // After the listings, since a new one can move the cursor onto
            // something else.
            self.sync_preview();
            self.collect_from_previewer();
            self.reap_detached();
            // Last of the pass, so a file asked for anywhere in it is opened
            // before the next frame is drawn.
            self.open_pending(terminal)?;
            self.toasts.retain(|toast| !toast.is_expired());
            self.tick = self.tick.wrapping_add(1);
        }

        // A placement belongs to the terminal rather than to the screen it was
        // made on, so leaving the alternate screen would leave it over the
        // shell.
        if let Err(error) = self.surface.clear(&mut io::stdout()) {
            tracing::warn!(%error, "the picture could not be taken back");
        }

        Ok(())
    }

    /// Brings the picture the terminal holds in line with the panel that was
    /// just drawn.
    ///
    /// The placement the frame settled and the bitmap it was settled for are
    /// held apart — one is the panel's arithmetic, the other the reader's
    /// answer — and are only ever put together here.
    fn place_preview(&mut self) {
        let wanted = match (self.preview_placement, &self.preview) {
            (Some(placement), Some(Content::Image(bitmap))) => Some(Wanted { placement, bitmap }),
            _ => None,
        };

        if let Err(error) = self.surface.reconcile(wanted, &mut io::stdout()) {
            tracing::warn!(%error, "the picture could not be placed");
        }
    }

    fn panel(&self, side: Side) -> &Panel {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    fn panel_mut(&mut self, side: Side) -> &mut Panel {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    pub fn get_focused_tabs(&self) -> &TabList {
        self.panel(self.focused_side).tabs()
    }

    pub fn get_focused_tabs_mut(&mut self) -> &mut TabList {
        self.panel_mut(self.focused_side).tabs_mut()
    }

    pub fn get_focused_pane(&self) -> &Pane {
        self.panel(self.focused_side).active_pane()
    }

    pub fn get_focused_pane_mut(&mut self) -> &mut Pane {
        self.panel_mut(self.focused_side).active_pane_mut()
    }

    fn draw(&mut self, frame: &mut Frame) {
        // Cleared every pass and set again only by a panel that draws a
        // picture, so a panel that stopped wanting one says so by saying
        // nothing.
        self.preview_placement = None;

        let main_layout = Layout::new(
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(2)],
        )
        .flex(Flex::Start)
        .split(frame.area());

        let layout = Layout::new(
            Direction::Horizontal,
            [Constraint::Percentage(50), Constraint::Percentage(50)],
        )
        .split(main_layout[0]);

        match self.opposite {
            Opposite::Listing => {
                let focused = self.focused_side;
                self.left
                    .tabs_mut()
                    .render(frame, layout[0], focused == Side::Left);
                self.right
                    .tabs_mut()
                    .render(frame, layout[1], focused == Side::Right);
            }
            // The preview sits opposite the cursor, so it follows a change of
            // side without anything having to be told about it. The other
            // panel is only hidden: it keeps its directory, which is what a
            // copy still goes into.
            Opposite::Preview => {
                let preview = PreviewPane::new(
                    self.previewing.as_deref(),
                    self.preview.as_ref(),
                    self.surface.graphics(),
                );
                // Where the panel wants a picture the terminal draws itself.
                // It comes back from the widget rather than being worked out
                // here: fitting a bitmap to an area is the same arithmetic
                // whichever of the two backends ends up drawing it.
                let mut area = None;
                match self.focused_side {
                    Side::Left => {
                        self.left.tabs_mut().render(frame, layout[0], true);
                        frame.render_stateful_widget(&preview, layout[1], &mut area);
                    }
                    Side::Right => {
                        frame.render_stateful_widget(&preview, layout[0], &mut area);
                        self.right.tabs_mut().render(frame, layout[1], true);
                    }
                }

                self.preview_placement = area.map(|area| Placement {
                    image: self.preview_generation,
                    area,
                });
            }
        }

        // Every overlay covers the whole screen, and a placement sits over the
        // text rather than under it. Wanting no picture is what takes one off
        // the screen; the surface does the rest of it.
        if !matches!(self.mode, Mode::Browse) || self.help.is_some() {
            self.preview_placement = None;
        }

        if let Mode::Confirm { dialog, .. } = &self.mode {
            frame.render_widget(dialog, frame.area());
        }

        if let Mode::Input { prompt, .. } = &self.mode {
            frame.render_widget(prompt, frame.area());
            frame.set_cursor_position(prompt.cursor_screen_position(frame.area()));
        }

        if let Mode::Find { finder } = &mut self.mode {
            // The cursor is read before the widget is handed over, since
            // drawing it takes the overlay by mutable reference.
            let area = frame.area();
            let cursor = finder.cursor_screen_position(area);
            frame.render_widget(finder, area);
            frame.set_cursor_position(cursor);
        }

        // Newest toast in the bottom-right corner, older ones stacked above it.
        // The first that no longer fits ends the stack; nothing older would fit
        // either.
        let toast_area = main_layout[0];
        let mut offset = 0;
        for toast in self.toasts.iter().rev() {
            let width = toast.width(toast_area.width);
            let height = toast.height(width);
            if offset + height > toast_area.height {
                break;
            }

            frame.render_widget(
                toast,
                Rect {
                    x: toast_area.right() - width,
                    y: toast_area.bottom() - offset - height,
                    width,
                    height,
                },
            );
            offset += height;
        }

        // The top of the two rows is reserved for the info bar whether or not it
        // has anything to draw.
        let [info_area, keys_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(main_layout[1]);

        // Counts the marks of the focused tab; marks of the other side and of
        // inactive tabs are not included.
        let marked = self
            .get_focused_tabs()
            .active_tab()
            .get_selected_items()
            .len();
        let progress = self.progress.as_ref().map(|progress| ProgressView {
            ratio: progress.ratio(),
            current: &progress.current,
            done: progress.items_done,
            total: progress.items_total,
        });
        frame.render_widget(
            InfoBar::new()
                .progress(progress)
                .marked(marked)
                .queued(self.worker.queued())
                .cancel_key(self.cancel_key)
                .tick(self.tick),
            info_area,
        );

        // The bar answers "what do the keys do right now", so while the help
        // is open it is the help's own keys rather than the mode's.
        if self.help.is_some() {
            frame.render_widget(Keybar::new(help::HELP_KEYS), keys_area);
        } else {
            // The help hint goes into every mode's row, since the key behind it
            // works in every mode.
            match self.mode {
                Mode::Browse => frame.render_widget(
                    Keybar::new(keys::BROWSE_KEYS).help_key(self.help_key),
                    keys_area,
                ),
                Mode::Confirm { .. } => frame.render_widget(
                    Keybar::new(Dialog::DIALOG_KEYS).help_key(self.help_key),
                    keys_area,
                ),
                Mode::Input { .. } => frame.render_widget(
                    Keybar::new(Prompt::PROMPT_KEYS).help_key(self.help_key),
                    keys_area,
                ),
                Mode::Find { .. } => frame.render_widget(
                    Keybar::new(Finder::FIND_KEYS).help_key(self.help_key),
                    keys_area,
                ),
            }
        }

        if let Some(state) = &mut self.help {
            let area = frame.area();
            let globals = keys::GLOBAL_KEYS;
            match self.mode {
                Mode::Browse => {
                    frame.render_widget(Help::new(keys::BROWSE_KEYS, globals, state), area)
                }
                Mode::Confirm { .. } => {
                    frame.render_widget(Help::new(Dialog::DIALOG_KEYS, globals, state), area)
                }
                Mode::Input { .. } => {
                    frame.render_widget(Help::new(Prompt::PROMPT_KEYS, globals, state), area);
                }
                Mode::Find { .. } => {
                    frame.render_widget(Help::new(Finder::FIND_KEYS, globals, state), area);
                }
            }
        }
    }

    /// Waits up to `TICK` for an event and handles it. Returns without reading
    /// when the wait times out, so the loop keeps turning while no key is
    /// pressed.
    fn handle_events(&mut self) -> Result<(), AppError> {
        if !event::poll(Self::TICK)? {
            return Ok(());
        }

        let Event::Key(key) = event::read()? else {
            return Ok(());
        };
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        tracing::info!("{}", key.modifiers);
        tracing::info!("{}", key.code);

        // The overlay answers first, so the key that opened it closes it again
        // rather than opening what is already open.
        if let Some(state) = &mut self.help {
            match keys::resolve(help::HELP_KEYS, &key) {
                Some(HelpMsg::Scroll(dir)) => state.scroll(dir),
                Some(HelpMsg::ScrollPage(dir)) => state.scroll_page(dir),
                Some(HelpMsg::Close) | None => self.help = None,
            }
            return Ok(());
        }

        if let Some(msg) = keys::resolve(keys::GLOBAL_KEYS, &key) {
            match msg {
                GlobalMsg::ShowHelp => self.help = Some(HelpState::default()),
            }
            return Ok(());
        }

        match &mut self.mode {
            Mode::Confirm { .. } => self.handle_confirm_key(key),
            Mode::Input { .. } => self.handle_input_key(key),
            Mode::Find { .. } => self.handle_find_key(key),
            Mode::Browse => {
                if let Some(action) = keys::resolve(keys::BROWSE_KEYS, &key)
                    && let Err(e) = self.dispatch(action)
                {
                    self.notify(ToastLevel::Error, e, None);
                }
            }
        }
        Ok(())
    }

    fn dispatch(&mut self, action: Action) -> Result<(), AppError> {
        match action {
            Action::Quit => self.confirm_quit(),
            Action::CancelJob => {
                self.worker.cancel();
                Ok(())
            }
            Action::ToggleSide => {
                self.focused_side = self.focused_side.toggle();
                Ok(())
            }
            Action::MoveCursor(nav_dir) => {
                self.move_cursor(nav_dir);
                Ok(())
            }
            Action::MoveCursorTo(end) => {
                match end {
                    ListEnd::First => self.get_focused_pane_mut().select_first(),
                    ListEnd::Last => self.get_focused_pane_mut().select_last(),
                }
                Ok(())
            }
            Action::ToggleMark => self
                .get_focused_tabs_mut()
                .active_tab_mut()
                .apply_mark(MarkOp::Toggle)
                .map_err(AppError::from),
            Action::ToggleTab(tog_dir) => {
                self.get_focused_tabs_mut().toggle(tog_dir);
                Ok(())
            }
            Action::MarkAndMove { op, nav_dir } => {
                self.get_focused_tabs_mut()
                    .active_tab_mut()
                    .apply_mark(op)?;
                self.move_cursor(nav_dir);
                Ok(())
            }
            Action::ClearMarks => {
                self.get_focused_tabs_mut()
                    .active_tab_mut()
                    .deselect_items();
                Ok(())
            }
            Action::OpenSelected => self
                .get_focused_pane_mut()
                .change_directory()
                .map_err(AppError::from),
            Action::Open(opener) => self.open_selected(opener),
            Action::NewTab => {
                if let Ok(new_tab) = Tab::new(String::from("New Tab")) {
                    self.get_focused_tabs_mut().add_tab(new_tab);
                }
                Ok(())
            }
            Action::CloseTab => {
                self.get_focused_tabs_mut().close_active();
                Ok(())
            }
            Action::Transfer { op } => self.queue_transfer(op),
            Action::Delete => self.confirm_delete(),
            Action::Rename => self.prompt_rename(),
            Action::CreateEntry => self.prompt_create_entry(),
            Action::RenameTab => self.prompt_rename_tab(),
            Action::Find => self.open_finder(),
            Action::ToggleQuickView => {
                self.opposite = self.opposite.toggle();
                Ok(())
            }
        }
    }

    /// Carries out what a dialog was opened to ask about. The counterpart of
    /// [`App::dispatch`] for the half that no longer asks.
    fn commit(&mut self, target: ConfirmTarget) -> Result<(), AppError> {
        match target {
            ConfirmTarget::Delete => self.queue_delete(),
            ConfirmTarget::Quit => {
                self.exit = true;
                Ok(())
            }
        }
    }

    fn move_cursor(&mut self, dir: VerticalDir) {
        match dir {
            VerticalDir::Up => self.get_focused_pane_mut().select_prev(),
            VerticalDir::Down => self.get_focused_pane_mut().select_next(),
        }
    }

    /// Puts down the entry under the cursor for `opener`, to be handed over at
    /// the end of the pass.
    ///
    /// A directory is turned away on its listed kind, which costs no read. A
    /// symlink is let through: what it points at is known only once it is
    /// followed, and the program doing the opening is what follows it.
    fn open_selected(&mut self, opener: Opener) -> Result<(), AppError> {
        let entry = self.get_focused_pane().selected_entry()?;
        let (kind, path) = (entry.kind, Arc::clone(&entry.path));

        if matches!(kind, DirEntryKind::Directory | DirEntryKind::Parent) {
            self.notify(ToastLevel::Warning, "That is a directory", None);
            return Ok(());
        }

        self.start(opener, path);
        Ok(())
    }

    /// Sends what the user entered to the program that belongs to it.
    ///
    /// Two rules, which is all a table of associations comes to here: text goes
    /// to the editor, anything else to whatever the system opens it with.
    /// A fifo, a socket or a device goes nowhere — a program handed one waits
    /// on it for as long as nobody is at the other end.
    fn open_entered(&mut self, path: Arc<Path>, kind: Kind) {
        let opener = match kind {
            Kind::Text => Opener::Edit,
            Kind::Opaque => Opener::System,
            Kind::NotAFile => {
                self.notify(ToastLevel::Warning, "That is not a file", None);
                return;
            }
        };

        self.start(opener, path);
    }

    /// Starts `opener` on `path`, either way it starts. A program that wants
    /// the terminal is put down for the end of the pass; one that does not is
    /// started here and never waited for.
    fn start(&mut self, opener: Opener, path: Arc<Path>) {
        let program = opener.program();

        match opener.launch() {
            Launch::Handover => self.opening = Some(Opening { program, path }),
            Launch::Detached => match open::detach(&program, &path) {
                Ok(child) => self.detached.push(child),
                Err(e) => self.notify(ToastLevel::Error, format!("{program}: {e}"), None),
            },
        }
    }

    /// Lets go of the detached programs that have ended.
    ///
    /// Nothing waits for one, and a child nobody ever asks about stays in the
    /// process table after it exits. Asking here costs a `waitpid` that returns
    /// at once.
    fn reap_detached(&mut self) {
        self.detached.retain_mut(|child| match child.try_wait() {
            Ok(None) => true,
            // Ended, or no longer answerable. Either way there is nothing left
            // to wait for.
            Ok(Some(_)) | Err(_) => false,
        });
    }

    /// Hands the terminal over when an action asked for it, and turns what came
    /// of the program into a toast.
    ///
    /// The error that leaves here is the terminal's own and ends the loop:
    /// there is nothing left to draw on. A program that would not start, or
    /// that started and complained, is news for the user rather than the end of
    /// the app.
    fn open_pending(&mut self, terminal: &mut DefaultTerminal) -> Result<(), AppError> {
        let Some(opening) = self.opening.take() else {
            return Ok(());
        };

        let Opening { program, path } = opening;
        match open::hand_over(terminal, &mut self.surface, &program, &path)? {
            Handover::Exited(status) if status.success() => (),
            // A program killed by a signal has no code of its own. Ctrl-C in a
            // program that reads keys the plain way is the ordinary way here.
            Handover::Exited(status) => match status.code() {
                Some(code) => self.notify(
                    ToastLevel::Warning,
                    format!("{program} exited with {code}"),
                    None,
                ),
                None => self.notify(ToastLevel::Warning, format!("{program} was killed"), None),
            },
            Handover::NotStarted(e) => {
                self.notify(ToastLevel::Error, format!("{program}: {e}"), None)
            }
        }

        Ok(())
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        let Some(msg) = keys::resolve(Dialog::DIALOG_KEYS, &key) else {
            return;
        };
        let Mode::Confirm { dialog, pending } = &mut self.mode else {
            return;
        };

        match msg {
            DialogMsg::Toggle => dialog.toggle(),
            DialogMsg::Cancel => self.mode = Mode::Browse,
            DialogMsg::Confirm => {
                let choice = dialog.choice();
                let pending = *pending;
                self.mode = Mode::Browse;
                if choice == Choice::Yes
                    && let Err(e) = self.commit(pending)
                {
                    self.notify(ToastLevel::Error, e, None);
                }
            }
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        let Mode::Input { prompt, pending } = &mut self.mode else {
            return;
        };

        match keys::resolve(Prompt::PROMPT_KEYS, &key) {
            Some(InputMsg::MoveCursor(dir)) => prompt.move_cursor(dir),
            Some(InputMsg::Cancel) => self.mode = Mode::Browse,
            Some(InputMsg::Confirm) => {
                let text = prompt.text().to_string();
                let pending = pending.clone();
                self.mode = Mode::Browse;
                if text.is_empty() {
                    self.notify(ToastLevel::Warning, "Nothing was typed", None);
                } else {
                    let result = match pending {
                        InputTarget::Mutation(mutation) => self.mutate(mutation, text),
                        InputTarget::RenameTab => {
                            self.get_focused_tabs_mut().active_tab_mut().rename(text);
                            Ok(())
                        }
                    };
                    if let Err(e) = result {
                        self.notify(ToastLevel::Error, e, None);
                    }
                }
            }
            None => match key.code {
                KeyCode::Char(c) => prompt.insert(c),
                KeyCode::Backspace => prompt.backspace(),
                _ => (),
            },
        }
    }

    /// Opens the find overlay over the directory of the focused panel. The
    /// root is fixed here and travels with the overlay, so walking the panel
    /// away while the search runs cannot move where it looks.
    fn open_finder(&mut self) -> Result<(), AppError> {
        let root = Arc::clone(self.get_focused_pane().get_current_dir());
        self.mode = Mode::Find {
            finder: Finder::new(root),
        };
        Ok(())
    }

    fn handle_find_key(&mut self, key: KeyEvent) {
        let Mode::Find { finder } = &mut self.mode else {
            return;
        };

        // Carries the query out of the borrow of the overlay, so the walk
        // behind it is replaced once the overlay is done being touched.
        let mut changed = None;

        match keys::resolve(Finder::FIND_KEYS, &key) {
            Some(FindMsg::MoveSelection(dir)) => finder.move_selection(dir),
            Some(FindMsg::MoveCursor(dir)) => finder.move_cursor(dir),
            Some(FindMsg::Cancel) => {
                self.reader.cancel();
                self.mode = Mode::Browse;
            }
            Some(FindMsg::Confirm) => {
                // Nothing to go to leaves the overlay open: the search is still
                // running and the next hit may be the one.
                let Some(target) = finder.selected().map(|hit| hit.path.to_path_buf()) else {
                    return;
                };
                self.reader.cancel();
                self.mode = Mode::Browse;

                self.get_focused_pane_mut().reveal(&target);
            }
            None => {
                match key.code {
                    KeyCode::Char(c) => finder.insert(c),
                    KeyCode::Backspace => finder.backspace(),
                    _ => return,
                }
                // The hits of the previous query describe a query nobody is
                // looking at any more.
                finder.restart();
                changed = Some((Arc::clone(finder.root()), finder.query().to_string()));
            }
        }

        let Some((root, query)) = changed else {
            return;
        };
        // An empty query would match every entry in the tree, so it searches
        // for nothing at all.
        if query.is_empty() {
            self.reader.cancel();
        } else if let Err(e) = self.reader.send(Search { root, query }) {
            self.notify(ToastLevel::Error, e, None);
        }
    }

    fn prompt_rename(&mut self) -> Result<(), AppError> {
        let entry = self.get_focused_pane().selected_entry()?;
        if entry.kind == DirEntryKind::Parent {
            return Ok(());
        }
        let target = Arc::clone(&entry.path);
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());

        let mut prompt = Prompt::new("Rename");
        if let Some(name) = name {
            prompt.set_text(name);
        }
        self.mode = Mode::Input {
            prompt,
            pending: InputTarget::Mutation(PendingMutation::Rename(target)),
        };
        Ok(())
    }

    fn prompt_create_entry(&mut self) -> Result<(), AppError> {
        let parent = Arc::clone(self.get_focused_pane().get_current_dir());
        self.mode = Mode::Input {
            prompt: Prompt::new("New"),
            pending: InputTarget::Mutation(PendingMutation::CreateEntry(parent)),
        };
        Ok(())
    }

    fn prompt_rename_tab(&mut self) -> Result<(), AppError> {
        let mut prompt = Prompt::new("Rename tab");
        prompt.set_text(self.get_focused_tabs().active_tab().get_title());
        self.mode = Mode::Input {
            prompt,
            pending: InputTarget::RenameTab,
        };
        Ok(())
    }

    fn mutate(&mut self, mutation: PendingMutation, name: String) -> Result<(), AppError> {
        let side = self.focused_side;
        let tab = self.get_focused_tabs().active_tab().id();
        // Renames and creations are instant, but they still go through the
        // queue: a rename of a directory a queued copy writes into has to
        // happen after that copy, not while it runs.
        let tag = self.worker.queue(Work::Mutate(mutation.op(name)))?;
        self.queued_marks.push(QueuedMarks { tag, side, tab });
        Ok(())
    }

    /// Takes the marks of the focused tab and hands them to the worker. The
    /// paths are copied out here and never read again, so the batch is settled
    /// the moment the key is pressed.
    fn queue_transfer(&mut self, op: TransferOp) -> Result<(), AppError> {
        let side = self.focused_side;
        let (items, to_dir, tab) = {
            let (from, to) = match side {
                Side::Left => (
                    self.left.tabs_mut().active_tab_mut(),
                    self.right.tabs_mut().active_tab_mut(),
                ),
                Side::Right => (
                    self.right.tabs_mut().active_tab_mut(),
                    self.left.tabs_mut().active_tab_mut(),
                ),
            };

            let to_dir = to.get_pane().get_current_dir().to_path_buf();
            let items: Vec<PathBuf> = from
                .get_selected_items()
                .iter()
                .map(|item| item.to_path_buf())
                .collect();
            let tab = from.id();

            // Marks go now rather than on success: the user keeps marking while
            // the batch runs, so there would be no telling ours from theirs by
            // the time it finishes. What fails comes back in the outcome.
            from.deselect_items();
            (items, to_dir, tab)
        };

        if items.is_empty() {
            self.notify(ToastLevel::Warning, "Nothing is marked", None);
            return Ok(());
        }

        let tag = self.worker.queue(Work::Transfer { op, items, to_dir })?;
        self.queued_marks.push(QueuedMarks { tag, side, tab });
        Ok(())
    }

    fn queue_delete(&mut self) -> Result<(), AppError> {
        let side = self.focused_side;
        let (items, tab) = {
            let tab = self.get_focused_tabs_mut().active_tab_mut();
            let items: Vec<PathBuf> = tab
                .get_selected_items()
                .iter()
                .map(|item| item.to_path_buf())
                .collect();
            let id = tab.id();
            tab.deselect_items();
            (items, id)
        };

        if items.is_empty() {
            self.notify(ToastLevel::Warning, "Nothing is marked", None);
            return Ok(());
        }

        let tag = self.worker.queue(Work::Delete { items })?;
        self.queued_marks.push(QueuedMarks { tag, side, tab });
        Ok(())
    }

    fn confirm_delete(&mut self) -> Result<(), AppError> {
        let tab = self.get_focused_tabs().active_tab();
        if tab.get_selected_items().is_empty() {
            self.notify(ToastLevel::Warning, "Nothing is marked", None);
            return Ok(());
        }
        let count = tab.get_selected_items().len();
        self.mode = Mode::Confirm {
            dialog: Dialog::new("Delete", format!("Delete {count} selected item(s)?")),
            pending: ConfirmTarget::Delete,
        };
        Ok(())
    }

    /// Asks before leaving with work still queued, since quitting drops it.
    fn confirm_quit(&mut self) -> Result<(), AppError> {
        if self.worker.is_idle() {
            self.exit = true;
            return Ok(());
        }

        let queued = self.worker.queued();
        self.mode = Mode::Confirm {
            dialog: Dialog::new(
                "Quit",
                format!("{queued} operation(s) still running. Quit?"),
            ),
            pending: ConfirmTarget::Quit,
        };
        Ok(())
    }

    /// Takes everything the worker has sent and folds it into the state the
    /// next frame draws from.
    fn collect_from_worker(&mut self) {
        let drained = self.worker.drain();

        if let Some(progress) = drained.progress {
            self.progress = Some(progress);
        }
        for outcome in drained.finished {
            self.finish_job(outcome);
        }

        if drained.health == Health::Stopped {
            self.queued_marks.clear();
            self.notify(
                ToastLevel::Error,
                "The background worker has stopped; restart Mula",
                None,
            );
        }

        // The last progress of a job describes a job that is over. Nothing is
        // left to draw once the queue empties.
        if self.worker.is_idle() {
            self.progress = None;
        }
    }

    /// Sends the listings the panes are waiting for.
    ///
    /// Which directory a pane shows is settled here once a pass rather than
    /// from each action that changes it, so a `cd`, a jump out of find, a new
    /// tab and a refresh after a job all take the same road. A pane whose
    /// request has already gone out asks for nothing.
    fn sync_listings(&mut self) {
        for side in [Side::Left, Side::Right] {
            let sent = self.panel_mut(side).send_wanted();
            if let Err(e) = sent {
                self.notify(ToastLevel::Error, e, None);
            }
        }
    }

    /// Takes the newest listing each side has been sent and hands it to the
    /// pane that asked for it. An older one describes a directory that pane has
    /// already left, and the reader has dropped it.
    fn collect_from_listings(&mut self) {
        for side in [Side::Left, Side::Right] {
            let collected = self.panel_mut(side).collect_listing();

            if collected.health == Health::Stopped {
                self.notify(
                    ToastLevel::Error,
                    "The background reader has stopped; restart Mula",
                    None,
                );
            }

            if let Some(e) = collected.error {
                self.notify(ToastLevel::Error, e, None);
            }

            if let Entered::Open { path, kind } = collected.entered {
                self.open_entered(path, kind);
            }
        }
    }

    /// Sends a preview request whenever what the panel would show has changed,
    /// and cancels the running one once there is nothing to show.
    ///
    /// The preview is a function of the focused side, its active tab and where
    /// the cursor sits, so it is settled here once a pass rather than from
    /// every action that could move one of them. One place cannot forget the
    /// cursor, a directory change, a tab, a side, or a refresh after a job.
    fn sync_preview(&mut self) {
        let wanted = match self.opposite {
            Opposite::Listing => None,
            Opposite::Preview => self
                .get_focused_pane()
                .selected_entry()
                .ok()
                .map(|entry| Arc::clone(&entry.path)),
        };

        if wanted == self.previewing {
            return;
        }

        // What is on screen belongs to the path that was under the cursor
        // before. Keeping it would draw one file's content under another
        // file's name, which the panel has no way of hedging.
        self.set_preview(None);
        match &wanted {
            Some(path) => {
                if let Err(e) = self.previewer.send(Preview {
                    path: Arc::clone(path),
                }) {
                    self.notify(ToastLevel::Error, e, None);
                }
            }
            None => self.previewer.cancel(),
        }
        self.previewing = wanted;
    }

    /// Takes the newest answer the previewer has sent. Unlike the hits of a
    /// search, these are snapshots of one panel: an older one is not a piece
    /// of the answer but an out of date whole, so only the last is kept.
    fn collect_from_previewer(&mut self) {
        let drained = self.previewer.drain();

        if drained.health == Health::Stopped {
            self.notify(
                ToastLevel::Error,
                "The background reader has stopped; restart Mula",
                None,
            );
        }

        if let Some(content) = drained.msgs.into_iter().next_back() {
            self.set_preview(Some(content));
        }
    }

    /// Replaces what the panel draws and gives it a number of its own.
    ///
    /// The number is what a placement is told apart by, so it has to turn on
    /// every replacement rather than on every change of path: the same file
    /// read twice is two pictures, and the second one has to reach the screen.
    fn set_preview(&mut self, content: Option<Content>) {
        self.preview = content;
        self.preview_generation = self.preview_generation.wrapping_add(1);
    }

    /// Takes the hits the reader has sent and appends them to the overlay that
    /// asked for them.
    fn collect_from_reader(&mut self) {
        let found = Found::fold(self.reader.drain());

        if found.health == Health::Stopped {
            self.notify(
                ToastLevel::Error,
                "The background reader has stopped; restart Mula",
                None,
            );
        }

        // Hits arriving with the overlay already closed have nowhere to go.
        // Closing it stops the walk, so this is only ever its tail.
        if let Mode::Find { finder } = &mut self.mode {
            finder.extend(found.hits);
            if let Some(ended) = found.ended {
                finder.finish(ended);
            }
        }
    }

    /// Reports a finished job and puts back the marks of whatever it could not
    /// handle, so a partial failure can be retried with one keypress.
    fn finish_job(&mut self, outcome: Outcome) {
        if let Some(index) = self
            .queued_marks
            .iter()
            .position(|queued| queued.tag == outcome.tag)
        {
            let queued = self.queued_marks.remove(index);
            if !outcome.failed.is_empty() {
                let tabs = self.panel_mut(queued.side).tabs_mut();
                if let Some(tab) = tabs.tab_mut(queued.tab) {
                    tab.mark_paths(outcome.failed);
                }
            }
        }

        let reason = outcome.reason.as_deref();
        match outcome.kind {
            JobKind::Transfer(TransferOp::Copy) => {
                self.notify_summary("copied", &outcome.summary, reason)
            }
            JobKind::Transfer(TransferOp::Move) => {
                self.notify_summary("moved", &outcome.summary, reason)
            }
            JobKind::Delete => self.notify_summary("deleted", &outcome.summary, reason),
            // A single mutation has nothing worth counting: it either happened,
            // or the error itself is the whole report.
            JobKind::Mutate => {
                if let Some(reason) = reason {
                    self.notify(ToastLevel::Error, reason, None);
                }
            }
        }

        // Only reads the disk when the job could have changed it, so a run that
        // skipped everything costs nothing.
        if outcome.summary.touched_disk() {
            self.refresh_panes();
        }
    }

    /// Asks the active pane on both sides for its listing again. Nothing is
    /// read here: the loop sends the requests and the answers arrive later, so
    /// the toast reporting a job is raised before the panes have caught up
    /// with what it did.
    fn refresh_panes(&mut self) {
        self.left.active_pane_mut().refresh();
        self.right.active_pane_mut().refresh();
    }

    /// Reports how a batch ended, taking the operation's past tense (`copied`)
    /// to build the message. A batch that touched nothing is a warning rather
    /// than an info, so silently doing nothing cannot read as success.
    ///
    /// `reason` is the last error the batch met. Counts alone say that
    /// something went wrong but never what, and the log is no help while the
    /// toast is still on screen.
    fn notify_summary(&mut self, verb: &str, summary: &ProcessedSummary, reason: Option<&str>) {
        if summary.total() == 0 {
            self.notify(ToastLevel::Warning, "Nothing is marked", None);
            return;
        }

        let mut message = if summary.nothing_processed() {
            format!("Nothing {verb}")
        } else if summary.processed() == summary.total() {
            format!("{} item(s) {verb}", summary.processed())
        } else {
            format!("{} of {} {verb}", summary.processed(), summary.total())
        };
        if summary.skipped() > 0 {
            message.push_str(&format!(", {} skipped", summary.skipped()));
        }
        if summary.has_failures() {
            message.push_str(&format!(", {} failed", summary.failed()));
            // One error cannot speak for several failures, so with more than
            // one it is offered as the latest rather than as the explanation.
            match (reason, summary.failed()) {
                (Some(reason), 1) => message.push_str(&format!(": {reason}")),
                (Some(reason), _) => message.push_str(&format!(", last: {reason}")),
                (None, _) => (),
            }
        }

        let level = if summary.has_failures() {
            ToastLevel::Error
        } else if summary.nothing_processed() {
            ToastLevel::Warning
        } else {
            ToastLevel::Info
        };
        self.notify(level, message, None);
    }

    fn notify(
        &mut self,
        level: ToastLevel,
        message: impl std::fmt::Display,
        duration: Option<Duration>,
    ) {
        let message = message.to_string();
        match level {
            ToastLevel::Error => tracing::error!("{message}"),
            ToastLevel::Warning => tracing::warn!("{message}"),
            ToastLevel::Info => tracing::info!("{message}"),
        }
        self.toasts.push_back(Toast::new(message, level, duration));
    }
}
