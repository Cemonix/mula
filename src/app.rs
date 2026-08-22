use std::{collections::VecDeque, io, path::Path, rc::Rc, time::Duration};

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
        ops::{MutationOp, TransferOp},
    },
    keys::{self, KeyBinding},
    ui::{
        dialog::{Choice, Dialog, DialogMsg},
        help::Help,
        infobar::InfoBar,
        keybar::Keybar,
        pane::{Pane, PaneError},
        prompt::{InputMsg, Prompt},
        tab::{MarkOp, Tab, TabList},
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
    Rename(Rc<Path>),
    CreateEntry(Rc<Path>),
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

/// How a batch of filesystem operations ended. `total` is the size of the
/// batch, fixed when it starts, so a run that stops early is still reported
/// against what it set out to do. A batch never fails as a whole: an item that
/// cannot be handled is counted, and the run carries on to the next one.
#[derive(Debug)]
pub struct ProcessedSummary {
    processed: usize,
    skipped: usize,
    failed: usize,
    total: usize,
}

impl ProcessedSummary {
    pub fn new(total: usize) -> Self {
        Self {
            processed: 0,
            skipped: 0,
            failed: 0,
            total,
        }
    }

    pub fn process(&mut self) {
        self.processed += 1;
    }

    pub fn skip(&mut self) {
        self.skipped += 1;
    }

    pub fn fail(&mut self) {
        self.failed += 1;
    }

    /// Whether nothing went through, either because nothing was marked or
    /// because every item was skipped or failed.
    pub fn nothing_processed(&self) -> bool {
        self.processed == 0
    }

    pub fn has_failures(&self) -> bool {
        self.failed > 0
    }

    /// Whether the disk may have changed. A failed item counts: an op can give
    /// up half way through and still leave something behind.
    pub fn touched_disk(&self) -> bool {
        self.processed > 0 || self.failed > 0
    }
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
            exit: false,
        })
    }

    /// Draws, waits up to `TICK` for a key, then drops the toasts that ran out.
    /// Errors that reach this far come from reading the terminal itself and end
    /// the loop; everything an action can fail at is turned into a toast by
    /// `handle_events`.
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<(), AppError> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
            self.toasts.retain(|toast| !toast.is_expired());
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
        frame.render_widget(InfoBar::new().marked(marked), info_area);

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
            Action::Quit => {
                self.exit = true;
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
            Action::Transfer { op } => {
                let summary = self.transfer(op);
                let verb = match op {
                    TransferOp::Copy => "copied",
                    TransferOp::Move => "moved",
                };
                self.notify_summary(verb, &summary);
                self.refresh_after(&summary)
            }
            Action::Delete => self.confirm_delete(),
            Action::DeleteMarked => {
                let summary = self.delete_marked();
                self.notify_summary("deleted", &summary);
                self.refresh_after(&summary)
            }
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
        let target = Rc::clone(&entry.path);
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
        let parent = Rc::clone(self.get_focused_pane().get_current_dir());
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
        mutation.op(name).execute()?;
        self.refresh_panes()
    }

    fn transfer(&mut self, op: TransferOp) -> ProcessedSummary {
        let (from, to) = match self.focused_side {
            Side::Left => (
                self.left_tabs.active_tab_mut(),
                self.right_tabs.active_tab_mut(),
            ),
            Side::Right => (
                self.right_tabs.active_tab_mut(),
                self.left_tabs.active_tab_mut(),
            ),
        };

        let to_dir = Rc::clone(to.get_pane().get_current_dir());

        let mut summary = ProcessedSummary::new(from.get_selected_items().len());
        for item in from.get_selected_items() {
            // The same directory on both sides is a skip, not a failure. A path
            // without a parent has nowhere to come from and skips too.
            if item.parent().is_none_or(|source| source == to_dir.as_ref()) {
                summary.skip();
                continue;
            }

            let Some(file_name) = item.file_name() else {
                summary.fail();
                tracing::error!(path = ?item, "no file name to transfer under");
                continue;
            };

            match op.execute(item, &to_dir.join(file_name)) {
                Ok(()) => summary.process(),
                Err(e) => {
                    summary.fail();
                    tracing::error!(path = ?item, error = %e, "transfer failed");
                }
            }
        }

        // Marks clear only when nothing failed and at least one item moved: a
        // batch that broke or changed nothing keeps its marks so it can be
        // retried or aimed somewhere else.
        if !summary.has_failures() && !summary.nothing_processed() {
            from.deselect_items();
        }

        summary
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

    fn delete_marked(&mut self) -> ProcessedSummary {
        let tab = self.get_focused_tabs_mut().active_tab_mut();

        let mut summary = ProcessedSummary::new(tab.get_selected_items().len());
        for item in tab.get_selected_items() {
            match (MutationOp::Delete {
                path: item.to_path_buf(),
            })
            .execute()
            {
                Ok(()) => summary.process(),
                // A mark can outlive the file it points at: marks survive a directory
                // change, and a retried batch walks over what the first pass removed.
                Err(e) if e.kind() == io::ErrorKind::NotFound => summary.skip(),
                Err(e) => {
                    summary.fail();
                    tracing::error!(path = ?item, error = %e, "delete failed");
                }
            }
        }

        // Marks clear only when nothing failed and at least one item went: a
        // batch that broke or changed nothing keeps its marks so it can be
        // retried.
        if !summary.has_failures() && !summary.nothing_processed() {
            tab.deselect_items();
        }

        summary
    }

    /// Refreshes both panes when a batch could have changed the disk, so a run
    /// that only skipped costs no reads.
    fn refresh_after(&mut self, summary: &ProcessedSummary) -> Result<(), AppError> {
        if summary.touched_disk() {
            self.refresh_panes()?;
        }
        Ok(())
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
    fn notify_summary(&mut self, verb: &str, summary: &ProcessedSummary) {
        if summary.total == 0 {
            self.notify(ToastLevel::Warning, "Nothing is marked", None);
            return;
        }

        let mut message = if summary.nothing_processed() {
            format!("Nothing {verb}")
        } else if summary.processed == summary.total {
            format!("{} item(s) {verb}", summary.processed)
        } else {
            format!("{} of {} {verb}", summary.processed, summary.total)
        };
        if summary.skipped > 0 {
            message.push_str(&format!(", {} skipped", summary.skipped));
        }
        if summary.has_failures() {
            message.push_str(&format!(", {} failed", summary.failed));
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
