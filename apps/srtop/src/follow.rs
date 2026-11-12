//! "Follow selected process" (PX-017; design §§6.2–6.3, 7.6, 8, 22.5, 22.7).
//!
//! Follow is an explicit server-side setting, published as a standard
//! `Toggle` and a status line. It is separate from ordinary selection
//! preservation: the client already keeps a selection on its item ID when the
//! rows are reordered, and never scrolls for it (§22.5, §22.7). Follow adds
//! exactly one thing on top: the table's standard `keep_visible` hint
//! (PX-017-G01) names the followed item, and the generic client scrolls the
//! least distance that shows it whenever a commit sets the hint or moves its
//! row. The hint is presentation guidance, never selection state, so follow
//! never changes, replaces or restores a selection: the selection is the
//! user's.
//!
//! - **Target.** The followed process is an item ID, which names one process
//!   instance (PX-003). A PID is never the target, so a process that reuses a
//!   followed PID is another item and is never followed.
//! - **Turning on** follows the user's own selection, as the server resolved
//!   it (the inspector's process): the hint is set to it, and nothing is
//!   promoted, because the client already shows it selected.
//! - **Sticky.** Follow stays on until the user turns it off or it stops by
//!   itself. Selecting another row (a click, the arrow keys or a search match)
//!   retargets follow, which sets the hint to that row. A sample, a sort or a
//!   filter that moves the followed row changes nothing on the wire for
//!   follow: the client reveals the row because the hinted item moved.
//! - **Panning.** Scrolling is local and the server never learns of it
//!   (§22.5). It never changes the selection and never stops follow. The view
//!   stays where the user put it until the followed process changes row.
//! - **One process.** While follow is on, the table's promoted `value` names
//!   the followed process or nothing: a retarget withdraws a promotion that
//!   names another process (PX-011-G01 keeps the user's later local
//!   selection), so a client that attaches later never restores a selection
//!   of a process follow is not on.
//! - **Stops.** When the followed process ends, or is no longer shown (a
//!   filter hid it, or the row limit left it out), follow turns itself off,
//!   its hint is cleared, and the status says which and why. It never moves
//!   to another row. A user who deselects the followed row (a
//!   `SELECTION_CHANGED` with no `value`, PX-017-G02) stops follow the same
//!   way, and turning follow on with nothing selected is refused.
//! - **Off.** No hint is held, and follow promotes nothing. Only an explicit
//!   search promotes, as before (PX-011).

use crate::bounded;
use crate::projection::Row;
use crate::source::Observed;
use srui_sdk::ItemId;

pub const FOLLOW_LABEL: &str = "Follow selected process";
pub const FOLLOW_DESCRIPTION: &str =
    "When on, the table scrolls to the selected process whenever a sample, a sort or a filter moves it to another row. Following never changes the selection. Scrolling yourself never turns follow off: the view stays where you put it until the process moves. Selecting another row follows that row. Follow stops, and says why, when the process ends or is no longer shown; a new process that reuses its PID is never followed.";
/// Follow is off: no sample moves the viewport.
pub const OFF_STATUS: &str =
    "Follow: off · the selection stays on its process and the table stays where you scroll it";
pub const UNCOLLECTED_FOLLOW_STATUS: &str = "Follow disabled: process collection not started";
/// Follow stopped because the user left the table with nothing selected
/// (PX-017-G02).
pub const SELECTION_CLEARED_FOLLOW_STATUS: &str =
    "Follow stopped: the selection was cleared; select a row and turn follow on again";
/// The longest follow status, before the cut mark.
pub const MAX_FOLLOW_STATUS_BYTES: usize = 512;
/// The most of a process name the status quotes.
const MAX_NAME_ECHO_BYTES: usize = 96;

/// The follow controls as a client holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowView {
    /// The toggle's value.
    pub checked: bool,
    pub status: String,
}

impl FollowView {
    pub(crate) fn initial(collecting: bool) -> Self {
        Self {
            checked: false,
            status: if collecting {
                OFF_STATUS
            } else {
                UNCOLLECTED_FOLLOW_STATUS
            }
            .to_string(),
        }
    }
}

#[derive(Debug, Clone)]
enum State {
    Off,
    /// Off, because follow was refused or stopped itself: the reason.
    Stopped(String),
    /// On: the followed instance's row as last published.
    On(Box<Row>),
}

/// The server's follow state.
#[derive(Debug, Clone)]
pub struct Follow {
    state: State,
}

impl Default for Follow {
    fn default() -> Self {
        Self { state: State::Off }
    }
}

fn pid_text(pid: &Observed<u32>) -> String {
    match pid {
        Observed::Known(pid) => format!("PID {pid}"),
        Observed::Missing(_) => "PID unavailable".to_string(),
    }
}

fn who(row: &Row) -> String {
    format!(
        "{} · {}",
        // A thread is named by its TID and its process's PID (PX-019).
        crate::threads::thread_subject(row).unwrap_or_else(|| pid_text(&row.key.pid)),
        bounded(row.display_name.as_str(), MAX_NAME_ECHO_BYTES)
    )
}

fn position(rows: &[Row], item: ItemId) -> Option<usize> {
    rows.iter().position(|row| row.item_id == item)
}

impl Follow {
    /// The followed process's item ID, while follow is on.
    pub fn following(&self) -> Option<ItemId> {
        self.followed().map(|row| row.item_id)
    }

    /// The followed process's row as last published, while follow is on.
    pub(crate) fn followed(&self) -> Option<&Row> {
        match &self.state {
            State::On(row) => Some(row),
            _ => None,
        }
    }

    /// Turns follow on for `selected`, the inspector's process: the user's own
    /// selection as the server resolved it. A selection that is not a shown
    /// row is refused with its reason, and follow stays off.
    pub(crate) fn turn_on(&mut self, selected: Option<&Row>, rows: &[Row], unfiltered: &[Row]) {
        let Some(selected) = selected else {
            self.state = State::Stopped(
                "Follow: off · nothing is selected; select a row, then turn follow on".into(),
            );
            return;
        };
        if let Some(index) = position(rows, selected.item_id) {
            self.state = State::On(Box::new(rows[index].clone()));
            return;
        }
        self.state = State::Stopped(match position(unfiltered, selected.item_id) {
            Some(_) => format!(
                "Follow: off · {} is not shown (filtered out or beyond the row limit); select a shown row first",
                who(selected)
            ),
            None => format!(
                "Follow: off · {} has ended; select a running process first",
                who(selected)
            ),
        });
    }

    pub(crate) fn turn_off(&mut self) {
        self.state = State::Off;
    }

    /// Keeps follow on the rows a client will hold, `rows`, out of the
    /// server-side collection `unfiltered`. Stops follow, never retargets it,
    /// when the process is not among `rows`. `user_threads` is whether user
    /// threads are shown (PX-019): a followed thread hidden with them says so.
    pub(crate) fn track(&mut self, rows: &[Row], unfiltered: &[Row], user_threads: bool) {
        let State::On(followed) = &mut self.state else {
            return;
        };
        let item = followed.item_id;
        if let Some(index) = position(rows, item) {
            **followed = rows[index].clone();
            return;
        }
        let why = match position(unfiltered, item) {
            Some(_) if followed.key.is_thread() && !user_threads => format!(
                "Follow stopped: {} is no longer shown: user threads are hidden; show them, select a row and turn follow on again",
                who(followed)
            ),
            Some(_) => format!(
                "Follow stopped: {} is no longer shown (filtered out or beyond the row limit); select a row and turn follow on again",
                who(followed)
            ),
            None => format!(
                "Follow stopped: {} has ended · a new process that reuses its PID is not followed; select a row and turn follow on again",
                who(followed)
            ),
        };
        self.state = State::Stopped(why);
    }

    /// Applies a selection while follow is on: `selected` is the shown row it
    /// resolved to, or `None` when it named no shown row, which stops follow.
    /// Another row becomes the followed process.
    pub(crate) fn selected(&mut self, selected: Option<&Row>) {
        let State::On(followed) = &mut self.state else {
            return;
        };
        match selected {
            Some(row) => **followed = row.clone(),
            None => {
                self.state = State::Stopped(
                    "Follow stopped: the selection named no row this server shows; select a row and turn follow on again"
                        .into(),
                );
            }
        }
    }

    /// The controls for a client that holds the process tree's preorder
    /// `rows` (PX-018). Off and stopped read as in the table. On, the status
    /// says where in the tree the followed process is, and when it is hidden
    /// under `hidden_under`, a parent the client reported collapsed: the
    /// tree's `keep_visible` hint never expands a parent, and a local expand
    /// never scrolls (PX-017-G01), so the user is told to expand it.
    pub(crate) fn view_tree(&self, rows: &[Row], hidden_under: Option<&Row>) -> FollowView {
        let State::On(followed) = &self.state else {
            return self.view(rows);
        };
        let status = match (hidden_under, position(rows, followed.item_id)) {
            (Some(parent), _) => format!(
                "Following {} · hidden under collapsed {}: expand it to see the followed process; the tree never expands it for you, and expanding never scrolls",
                who(followed),
                who(parent)
            ),
            (None, at) => format!(
                "Following {}{} · the tree scrolls to it whenever it moves to another row; following never changes the selection, and scrolling yourself never stops it",
                who(followed),
                at.map(|index| format!(" · item {} of {} in the tree", index + 1, rows.len()))
                    .unwrap_or_default()
            ),
        };
        FollowView {
            checked: true,
            status: bounded(&status, MAX_FOLLOW_STATUS_BYTES),
        }
    }

    /// The user left the table with nothing selected (PX-017-G02): follow,
    /// if on, stops and says so. It never moves to another row.
    pub(crate) fn cleared(&mut self) {
        if let State::On(_) = self.state {
            self.state = State::Stopped(SELECTION_CLEARED_FOLLOW_STATUS.into());
        }
    }

    /// The controls for a client that holds `rows`.
    pub(crate) fn view(&self, rows: &[Row]) -> FollowView {
        match &self.state {
            State::Off => FollowView::initial(true),
            State::Stopped(why) => FollowView {
                checked: false,
                status: bounded(why, MAX_FOLLOW_STATUS_BYTES),
            },
            State::On(followed) => {
                let at = position(rows, followed.item_id)
                    .map(|index| format!(" · row {} of {}", index + 1, rows.len()))
                    .unwrap_or_default();
                FollowView {
                    checked: true,
                    status: bounded(
                        &format!(
                            "Following {}{at} · the table scrolls to it whenever it moves to another row; following never changes the selection, and scrolling yourself never stops it",
                            who(followed)
                        ),
                        MAX_FOLLOW_STATUS_BYTES,
                    ),
                }
            }
        }
    }
}
