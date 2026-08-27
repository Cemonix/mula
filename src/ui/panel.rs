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

use std::io;

use crate::{
    fs::{
        directory::Directory,
        listing::Listing,
        reader::{Drained, Reader},
        worker::Health,
    },
    ui::{
        pane::{Pane, PaneError},
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
}

impl Panel {
    /// A panel of one tab, with its reader running and nothing asked for yet.
    pub fn new() -> Result<Self, PaneError> {
        Ok(Self {
            tabs: TabList::new(vec![Tab::new(String::from("New Tab"))?]),
            listings: Reader::<Listing>::start(()),
            listing_for: None,
        })
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
    /// for yet.
    ///
    /// One reader serves every tab, so a request replaces whatever was in
    /// flight. The tab that loses it goes back to wanting its listing, or it
    /// would sit on a request whose answer was thrown away and never ask
    /// again.
    pub fn send_wanted(&mut self) -> io::Result<()> {
        let id = self.tabs.active_tab().id();
        let Some(path) = self.active_pane_mut().take_unsent() else {
            return Ok(());
        };

        if let Some(previous) = self.listing_for.replace(id)
            && previous != id
            && let Some(tab) = self.tabs.tab_mut(previous)
        {
            tab.get_pane_mut().unsend();
        }

        self.listings.send(Listing { path }).map(|_| ())
    }

    /// Takes the newest listing the reader has sent and gives it to the tab
    /// that asked for it. Older ones describe a directory that tab has already
    /// left, and the reader has dropped them.
    pub fn collect_listing(&mut self) -> Collected {
        let Drained { msgs, health } = self.listings.drain();
        let error = msgs
            .into_iter()
            .next_back()
            .and_then(|listing| self.deliver(listing).err());

        Collected { health, error }
    }

    /// Gives a listing to the tab that asked for it. A tab closed while its
    /// answer was on its way has nowhere to put it, which is the whole of what
    /// closing a tab has to handle.
    fn deliver(&mut self, listing: io::Result<Directory>) -> Result<(), PaneError> {
        let Some(tab) = self.listing_for.take().and_then(|id| self.tabs.tab_mut(id)) else {
            return Ok(());
        };
        tab.get_pane_mut().listed(listing)
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    use std::{path::Path, sync::Arc};

    use crate::ui::tab::ToggleDirection;

    /// A panel of two tabs, each holding a listing and waiting for nothing,
    /// which is where a panel settles once its reads have arrived. The active
    /// tab is the one it started with.
    fn settled_panel() -> Panel {
        let mut panel = Panel::new().unwrap();
        panel
            .tabs
            .add_tab(Tab::new(String::from("second")).unwrap());

        for _ in 0..2 {
            let path = Arc::clone(panel.active_pane().get_current_dir());
            panel.active_pane_mut().take_unsent();
            panel
                .active_pane_mut()
                .listed(Ok(Directory::new(path, Vec::new())))
                .unwrap();
            panel.tabs.toggle(ToggleDirection::Next);
        }
        panel
    }

    fn listing(path: &str) -> Directory {
        Directory::new(Arc::from(Path::new(path)), Vec::new())
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
        panel.send_wanted().unwrap();
        // The cursor moves on before the answer lands, which is the whole of
        // the race: the tab on screen is no longer the tab that asked.
        panel.tabs.toggle(ToggleDirection::Next);
        let arrived_at = panel.tabs.active_tab().id();

        panel.deliver(Ok(listing("/somewhere"))).unwrap();

        assert_eq!(
            current_dir_of(&mut panel, asked).as_ref(),
            Path::new("/somewhere")
        );
        assert_ne!(
            current_dir_of(&mut panel, arrived_at).as_ref(),
            Path::new("/somewhere")
        );
    }

    #[test]
    fn a_tab_that_loses_the_reader_asks_again() {
        let mut panel = settled_panel();
        let first = panel.tabs.active_tab().id();

        panel.active_pane_mut().reveal(Path::new("/one/deep"));
        panel.send_wanted().unwrap();

        panel.tabs.toggle(ToggleDirection::Next);
        panel.active_pane_mut().reveal(Path::new("/two/deep"));
        panel.send_wanted().unwrap();

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
        panel.send_wanted().unwrap();
        panel.active_pane_mut().reveal(Path::new("/two/deep"));
        panel.send_wanted().unwrap();

        // Taking the reader back from itself would leave the tab wanting a
        // listing it has already asked for, and it would ask on every pass.
        let pane = panel.tabs.tab_mut(only).unwrap().get_pane_mut();
        assert!(pane.take_unsent().is_none());
    }
}
