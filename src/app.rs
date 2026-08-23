use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
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
    action::{Action, VerticalDir},
    fs::{
        directory::DirEntryKind,
        job::{JobKind, JobTag, Outcome, Progress, Work},
        ops::{MutationOp, ProcessedSummary, TransferOp},
        worker::{Health, Worker},
    },
    keys::{self, KeyBinding},
    ui::{
        dialog::{Choice, Dialog, DialogMsg},
        help::Help,
        infobar::{InfoBar, ProgressView},
        keybar::Keybar,
        pane::{Pane, PaneError},
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

#[derive(Debug)]
pub enum Mode {
    Browse,
    Confirm {
        dialog: Dialog,
        pending: Action,
    },
    Input {
        prompt: Prompt,
        pending: InputTarget,
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
    left_tabs: TabList,
    right_tabs: TabList,
    focused_side: Side,
    toasts: VecDeque<Toast>,
    mode: Mode,
    show_help: bool,
    help_key: Option<KeyBinding>,
    /// Resolved from the key table once, so the hint the info bar draws while a
    /// job runs cannot drift from the key that actually cancels it.
    cancel_key: Option<KeyBinding>,
    worker: Worker,
    /// The newest snapshot of the running job, or `None` while the queue is
    /// empty. Derived from the worker and true only while it works, so it
    /// belongs in the info bar rather than in a toast.
    progress: Option<Progress>,
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

    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            left_tabs: TabList::new(vec![Tab::new(String::from("New Tab"))?]),
            right_tabs: TabList::new(vec![Tab::new(String::from("New Tab"))?]),
            focused_side: Side::Left,
            toasts: VecDeque::new(),
            mode: Mode::Browse,
            show_help: false,
            help_key: keys::find(keys::BROWSE_KEYS, |a| matches!(a, Action::ShowHelp))
                .map(|binding| binding.key),
            cancel_key: keys::find(keys::BROWSE_KEYS, |a| matches!(a, Action::CancelJob))
                .map(|binding| binding.key),
            worker: Worker::start(),
            progress: None,
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
            self.handle_events()?;
            self.collect_from_worker();
            self.toasts.retain(|toast| !toast.is_expired());
            self.tick = self.tick.wrapping_add(1);
        }

        Ok(())
    }

    pub fn get_focused_tabs(&self) -> &TabList {
        match self.focused_side {
            Side::Left => &self.left_tabs,
            Side::Right => &self.right_tabs,
        }
    }

    pub fn get_focused_tabs_mut(&mut self) -> &mut TabList {
        match self.focused_side {
            Side::Left => &mut self.left_tabs,
            Side::Right => &mut self.right_tabs,
        }
    }

    pub fn get_focused_pane(&self) -> &Pane {
        match self.focused_side {
            Side::Left => self.left_tabs.active_tab().get_pane(),
            Side::Right => self.right_tabs.active_tab().get_pane(),
        }
    }

    pub fn get_focused_pane_mut(&mut self) -> &mut Pane {
        match self.focused_side {
            Side::Left => self.left_tabs.active_tab_mut().get_pane_mut(),
            Side::Right => self.right_tabs.active_tab_mut().get_pane_mut(),
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
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

        self.left_tabs
            .render(frame, layout[0], self.focused_side == Side::Left);
        self.right_tabs
            .render(frame, layout[1], self.focused_side == Side::Right);

        if let Mode::Confirm { dialog, .. } = &self.mode {
            frame.render_widget(dialog, frame.area());
        }

        if let Mode::Input { prompt, .. } = &self.mode {
            frame.render_widget(prompt, frame.area());
            frame.set_cursor_position(prompt.cursor_screen_position(frame.area()));
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

        match self.mode {
            Mode::Browse => frame.render_widget(
                Keybar::new(keys::BROWSE_KEYS).help_key(self.help_key),
                keys_area,
            ),
            Mode::Confirm { .. } => {
                frame.render_widget(Keybar::new(Dialog::DIALOG_KEYS), keys_area)
            }
            Mode::Input { .. } => {
                frame.render_widget(Keybar::new(Prompt::PROMPT_KEYS), keys_area);
            }
        }

        if self.show_help {
            match self.mode {
                Mode::Browse => frame.render_widget(Help::new(keys::BROWSE_KEYS), frame.area()),
                Mode::Confirm { .. } => {
                    frame.render_widget(Help::new(Dialog::DIALOG_KEYS), frame.area())
                }
                Mode::Input { .. } => {
                    frame.render_widget(Help::new(Prompt::PROMPT_KEYS), frame.area());
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

        if self.show_help {
            self.show_help = false;
            return Ok(());
        }

        match &mut self.mode {
            Mode::Confirm { .. } => self.handle_confirm_key(key),
            Mode::Input { .. } => self.handle_input_key(key),
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
            Action::QuitAnyway => {
                self.exit = true;
                Ok(())
            }
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
            Action::NewTab => {
                if let Ok(new_tab) = Tab::new(String::from("New Tab")) {
                    self.get_focused_tabs_mut().add_tab(new_tab);
                }
                Ok(())
            }
            Action::Transfer { op } => self.queue_transfer(op),
            Action::Delete => self.confirm_delete(),
            Action::DeleteMarked => self.queue_delete(),
            Action::Rename => self.prompt_rename(),
            Action::CreateEntry => self.prompt_create_entry(),
            Action::RenameTab => self.prompt_rename_tab(),
            Action::ShowHelp => {
                self.show_help = true;
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
                    && let Err(e) = self.dispatch(pending)
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
                    self.left_tabs.active_tab_mut(),
                    self.right_tabs.active_tab_mut(),
                ),
                Side::Right => (
                    self.right_tabs.active_tab_mut(),
                    self.left_tabs.active_tab_mut(),
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
            pending: Action::DeleteMarked,
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
            pending: Action::QuitAnyway,
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
                let tabs = match queued.side {
                    Side::Left => &mut self.left_tabs,
                    Side::Right => &mut self.right_tabs,
                };
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
        if outcome.summary.touched_disk()
            && let Err(e) = self.refresh_panes()
        {
            self.notify(ToastLevel::Error, e, None);
        }
    }

    /// Refreshes the active pane on both sides. Both run even when the first fails;
    /// the error of the left one is returned first.
    fn refresh_panes(&mut self) -> Result<(), AppError> {
        let left = self.left_tabs.active_tab_mut().get_pane_mut().refresh();
        let right = self.right_tabs.active_tab_mut().get_pane_mut().refresh();
        left.and(right).map_err(AppError::from)
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
