//! One half of the screen: its tabs, the reader that fills them, and which tab
//! the outstanding listing belongs to.
//!
//! The three travel together because they answer one question — *what is this
//! side showing* — and holding them as three pairs of `left_`/`right_` fields
//! on `App` turned every answer into a match on the side.
//!
//! What the panel owns and nobody outside it sees is the reader. One serves
//! every tab of the panel, so there is at most one listing in flight per side,
//! and the two rules that follow from that are the whole of this module: an
//! answer belongs to the tab that asked for it, and a tab that loses the
//! reader has to ask again.
//!
//! A second reader measures the directories the active tab shows. Its answers
//! are not the tab's but `App`'s, which keeps every total for both sides, so
//! it needs no rule about who asked.

use std::{collections::HashSet, io, path::Path, sync::Arc};

use rayon::ThreadPool;

use crate::{
    fs::{
        directory::Detail,
        listing::{Listed, Listing},
        reader::{Drained, Reader},
        sizes::{DirSizes, Sizing},
        worker::Health,
    },
    ui::{
        pane::{Entered, Pane, PaneError},
        tab::{Tab, TabId, TabList},
    },
};

/// What the panel's reader had to say on one pass of the loop.
#[derive(Debug)]
pub struct Collected {
    pub health: Health,
    /// The listing that could not be read, for the caller to report. A
    /// directory the pane merely stepped over is not one of these.
    pub error: Option<PaneError>,
    /// What the tab that asked wants done about the entry it entered.
    pub entered: Entered,
}

#[derive(Debug)]
pub struct Panel {
    tabs: TabList,
    /// Serves this panel alone. Not one reader for the role: `Reader` is last
    /// wins, and both panels are refreshed together after a job finishes, so a
    /// shared counter would drop one answer and leave that panel waiting for a
    /// generation that never comes.
    listings: Reader<Listing>,
    /// The tab whose listing is on its way. An answer goes to the tab that
    /// asked for it rather than to whichever one is active when it lands:
    /// switching or closing a tab while a read is in flight would otherwise
    /// drop one tab's listing into another tab's pane.
    listing_for: Option<TabId>,
    /// Measures the directories the active tab shows. A reader of its own:
    /// a walk takes seconds, and a listing queued behind it would too.
    sizing: Reader<Sizing>,
    /// What the running walk was asked for, or `None` when nothing was.
    measuring: Option<Measuring>,
}

/// The directories a panel sent to be measured, and where it stood.
#[derive(Debug)]
struct Measuring {
    dir: Arc<Path>,
    asked: HashSet<Arc<Path>>,
}

impl Panel {
    /// A panel of one tab, with its readers running and nothing asked for yet.
    /// Walks run on `pool`.
    pub fn new(pool: Arc<ThreadPool>) -> Result<Self, PaneError> {
        Ok(Self {
            tabs: TabList::new(vec![Tab::new(String::from("New Tab"))?]),
            listings: Reader::<Listing>::start(()),
            listing_for: None,
            sizing: Reader::<Sizing>::start(pool),
            measuring: None,
        })
    }

    /// Sends the directories the active tab shows to be measured.
    ///
    /// A tab standing somewhere new has all of them measured again, the ones
    /// `sizes` already knows included, since a total goes stale with any change
    /// below it. A tab that stays asks for the ones nobody knows and nobody
    /// has asked about yet; sending replaces the running walk, so those it was
    /// still on are asked again with them.
    pub fn send_sizing(&mut self, sizes: &DirSizes) -> io::Result<()> {
        let pane = self.active_pane();
        let here = Arc::clone(pane.get_current_dir());
        let mut dirs = pane.visible_directories();

        if let Some(measuring) = &self.measuring
            && measuring.dir == here
        {
            dirs.retain(|dir| sizes.get(dir).is_none());
            if dirs.iter().all(|dir| measuring.asked.contains(dir)) {
                return Ok(());
            }
        }

        self.measuring = Some(Measuring {
            dir: here,
            asked: dirs.iter().cloned().collect(),
        });
        self.sizing.send(Sizing { dirs }).map(|_| ())
    }

    /// Stops measuring and forgets what was asked, so the next
    /// [`Panel::send_sizing`] asks for everything missing.
    pub fn restart_sizing(&mut self) {
        self.sizing.cancel();
        if let Some(measuring) = &mut self.measuring {
            measuring.asked.clear();
        }
    }

    /// Stops measuring for a pane that has nothing to show the totals in.
    pub fn stop_sizing(&mut self) {
        if self.measuring.take().is_some() {
            self.sizing.cancel();
        }
    }

    /// Takes the totals the running walk has sent.
    pub fn collect_sizes(&mut self) -> Drained<Vec<(Arc<Path>, u64)>> {
        self.sizing.drain()
    }

    pub fn tabs(&self) -> &TabList {
        &self.tabs
    }

    pub fn tabs_mut(&mut self) -> &mut TabList {
        &mut self.tabs
    }

    pub fn active_pane(&self) -> &Pane {
        self.tabs.active_tab().get_pane()
    }

    pub fn active_pane_mut(&mut self) -> &mut Pane {
        self.tabs.active_tab_mut().get_pane_mut()
    }

    /// Hands the reader what the active tab is waiting for and has not asked
    /// for yet, reading each entry to `detail`, or to more when the order the
    /// tab is sorted in compares what `detail` leaves out.
    ///
    /// One reader serves every tab, so a request replaces whatever was in
    /// flight. The tab that loses it goes back to wanting its listing, or it
    /// would sit on a request whose answer was thrown away and never ask
    /// again.
    ///
    /// `detail` is asked of the active tab alone. A tab in the background
    /// draws nothing, and the pass that makes it active is the pass that finds
    /// its listing too thin for the columns and reads it again.
    pub fn send_wanted(&mut self, detail: Detail) -> io::Result<()> {
        let id = self.tabs.active_tab().id();
        let detail = detail.max(self.active_pane().sort().detail());
        self.active_pane_mut().want_detail(detail);
        let Some(path) = self.active_pane_mut().take_unsent() else {
            return Ok(());
        };

        if let Some(previous) = self.listing_for.replace(id)
            && previous != id
            && let Some(tab) = self.tabs.tab_mut(previous)
        {
            tab.get_pane_mut().unsend();
        }

        self.listings.send(Listing { path, detail }).map(|_| ())
    }

    /// Takes the newest listing the reader has sent and gives it to the tab
    /// that asked for it. Older ones describe a directory that tab has already
    /// left, and the reader has dropped them.
    pub fn collect_listing(&mut self) -> Collected {
        let Drained { msgs, health } = self.listings.drain();
        let (entered, error) = match msgs.into_iter().next_back() {
            Some(listing) => match self.deliver(listing) {
                Ok(entered) => (entered, None),
                Err(e) => (Entered::Nothing, Some(e)),
            },
            None => (Entered::Nothing, None),
        };

        Collected {
            health,
            error,
            entered,
        }
    }

    /// Gives a listing to the tab that asked for it. A tab closed while its
    /// answer was on its way has nowhere to put it, which is the whole of what
    /// closing a tab has to handle.
    fn deliver(&mut self, listing: io::Result<Listed>) -> Result<Entered, PaneError> {
        let Some(tab) = self.listing_for.take().and_then(|id| self.tabs.tab_mut(id)) else {
            return Ok(Entered::Nothing);
        };
        tab.get_pane_mut().listed(listing)
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    use std::{path::Path, sync::Arc};

    use rayon::ThreadPoolBuilder;

    use crate::{
        fs::directory::{DirEntry, DirEntryKind, Directory},
        ui::tab::ToggleDirection,
    };

    fn panel() -> Panel {
        let pool = ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        Panel::new(Arc::new(pool)).unwrap()
    }

    /// A panel of two tabs, each holding a listing and waiting for nothing,
    /// which is where a panel settles once its reads have arrived. The active
    /// tab is the one it started with.
    fn settled_panel() -> Panel {
        let mut panel = panel();
        panel
            .tabs
            .add_tab(Tab::new(String::from("second")).unwrap());

        for _ in 0..2 {
            let path = Arc::clone(panel.active_pane().get_current_dir());
            panel.active_pane_mut().take_unsent();
            panel
                .active_pane_mut()
                .listed(Ok(Listed::Directory(Directory::new(
                    path,
                    Vec::new(),
                    Detail::NamesOnly,
                ))))
                .unwrap();
            panel.tabs.toggle(ToggleDirection::Next);
        }
        panel
    }

    fn listing(path: &str) -> Directory {
        Directory::new(Arc::from(Path::new(path)), Vec::new(), Detail::NamesOnly)
    }

    fn current_dir_of(panel: &mut Panel, tab: TabId) -> Arc<Path> {
        Arc::clone(
            panel
                .tabs
                .tab_mut(tab)
                .unwrap()
                .get_pane()
                .get_current_dir(),
        )
    }

    #[test]
    fn a_listing_goes_to_the_tab_that_asked_for_it() {
        let mut panel = settled_panel();
        let asked = panel.tabs.active_tab().id();

        panel.active_pane_mut().reveal(Path::new("/somewhere/deep"));
        panel.send_wanted(Detail::NamesOnly).unwrap();
        // The cursor moves on before the answer lands, which is the whole of
        // the race: the tab on screen is no longer the tab that asked.
        panel.tabs.toggle(ToggleDirection::Next);
        let arrived_at = panel.tabs.active_tab().id();

        panel
            .deliver(Ok(Listed::Directory(listing("/somewhere"))))
            .unwrap();

        assert_eq!(
            current_dir_of(&mut panel, asked).as_ref(),
            Path::new("/somewhere")
        );
        assert_ne!(
            current_dir_of(&mut panel, arrived_at).as_ref(),
            Path::new("/somewhere")
        );
    }

    /// The columns are drawn for the tab on screen. A tab behind it pays for
    /// them on the pass that brings it forward, not on the one that turns them
    /// on.
    #[test]
    fn a_tab_reads_its_directory_again_once_the_columns_reach_it() {
        let mut panel = settled_panel();
        let behind = panel.tabs.active_tab().id();
        panel.tabs.toggle(ToggleDirection::Next);
        let front = panel.tabs.active_tab().id();

        panel.send_wanted(Detail::WithMetadata).unwrap();

        assert_eq!(panel.listing_for, Some(front));
        assert!(
            panel
                .tabs
                .tab_mut(behind)
                .unwrap()
                .get_pane_mut()
                .take_unsent()
                .is_none()
        );

        panel.tabs.toggle(ToggleDirection::Previous);
        panel.send_wanted(Detail::WithMetadata).unwrap();

        assert_eq!(panel.listing_for, Some(behind));
    }

    #[test]
    fn a_tab_that_loses_the_reader_asks_again() {
        let mut panel = settled_panel();
        let first = panel.tabs.active_tab().id();

        panel.active_pane_mut().reveal(Path::new("/one/deep"));
        panel.send_wanted(Detail::NamesOnly).unwrap();

        panel.tabs.toggle(ToggleDirection::Next);
        panel.active_pane_mut().reveal(Path::new("/two/deep"));
        panel.send_wanted(Detail::NamesOnly).unwrap();

        // The first answer went out under a generation that has been raised
        // since, so it is never coming; the tab has to want its listing again.
        let pane = panel.tabs.tab_mut(first).unwrap().get_pane_mut();
        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/one")));
    }

    #[test]
    fn asking_twice_from_one_tab_does_not_take_the_reader_from_itself() {
        let mut panel = settled_panel();
        let only = panel.tabs.active_tab().id();

        panel.active_pane_mut().reveal(Path::new("/one/deep"));
        panel.send_wanted(Detail::NamesOnly).unwrap();
        panel.active_pane_mut().reveal(Path::new("/two/deep"));
        panel.send_wanted(Detail::NamesOnly).unwrap();

        // Taking the reader back from itself would leave the tab wanting a
        // listing it has already asked for, and it would ask on every pass.
        let pane = panel.tabs.tab_mut(only).unwrap().get_pane_mut();
        assert!(pane.take_unsent().is_none());
    }

    /// Puts the active tab in `path`, holding one directory per name.
    fn stand_in(panel: &mut Panel, path: &str, names: &[&str]) {
        let root: Arc<Path> = Arc::from(Path::new(path));
        let entries = names
            .iter()
            .map(|name| DirEntry {
                path: Arc::from(root.join(name).as_path()),
                kind: DirEntryKind::Directory,
                meta: None,
            })
            .collect();
        let pane = panel.active_pane_mut();
        pane.jump(Arc::clone(&root));
        pane.take_unsent();
        pane.listed(Ok(Listed::Directory(Directory::new(
            root,
            entries,
            Detail::NamesOnly,
        ))))
        .unwrap();
    }

    fn asked(panel: &Panel) -> HashSet<String> {
        panel
            .measuring
            .as_ref()
            .unwrap()
            .asked
            .iter()
            .map(|dir| dir.display().to_string())
            .collect()
    }

    fn known(dirs: &[&str]) -> DirSizes {
        let mut sizes = DirSizes::default();
        for dir in dirs {
            sizes.insert(Arc::from(Path::new(dir)), 1);
        }
        sizes
    }

    /// A total goes stale with any change below it, and entering a directory
    /// is the moment to find out.
    #[test]
    fn a_tab_somewhere_new_measures_every_directory_again() {
        let mut panel = panel();
        stand_in(&mut panel, "/a", &["x", "y"]);

        panel.send_sizing(&known(&["/a/x"])).unwrap();

        assert_eq!(asked(&panel), HashSet::from(["/a/x".into(), "/a/y".into()]));
    }

    #[test]
    fn a_tab_that_stays_asks_only_for_what_nobody_knows() {
        let mut panel = panel();
        stand_in(&mut panel, "/a", &["x", "y"]);
        panel.send_sizing(&DirSizes::default()).unwrap();
        panel.restart_sizing();

        panel.send_sizing(&known(&["/a/x"])).unwrap();

        assert_eq!(asked(&panel), HashSet::from(["/a/y".into()]));
    }

    /// Sending replaces the running walk, so asking again for what it is
    /// already on would start it over on every pass of the loop.
    #[test]
    fn a_tab_that_stays_does_not_ask_twice() {
        let mut panel = panel();
        stand_in(&mut panel, "/a", &["x", "y"]);
        panel.send_sizing(&DirSizes::default()).unwrap();
        let generation = panel.sizing.send(Sizing { dirs: Vec::new() }).unwrap();

        panel.send_sizing(&known(&["/a/x"])).unwrap();

        assert_eq!(
            panel.sizing.send(Sizing { dirs: Vec::new() }).unwrap(),
            generation + 1
        );
    }
}
