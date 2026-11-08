//! "Follow selected process" (PX-017; design §§6.2–6.3, 7.6, 8, 22.5, 22.7).
//!
//! Follow is an explicit server-side setting, published as a standard
//! `Toggle` and a status line. It is separate from ordinary selection
//! preservation: the client already keeps a selection on its item ID when the
//! rows are reordered, and never scrolls for it (§22.5, §22.7). Follow adds
//! exactly one thing on top: when the followed process moves to another row,
//! the server promotes its item again (PX-011-G01), and the generic client
//! selects it and scrolls it into view once. Nothing else scrolls.
//!
//! - **Target.** The followed process is an item ID, which names one process
//!   instance (PX-003). A PID is never the target, so a process that reuses a
//!   followed PID is another item and is never followed.
//! - **Sticky.** Follow stays on until the user turns it off or it stops by
//!   itself. Selecting another row (a click, the arrow keys or a search match)
//!   follows that row instead.
//! - **Panning.** Scrolling is local and the server never learns of it
//!   (§22.5). It never changes the selection and never stops follow. The view
//!   stays where the user put it until the followed process changes row; then
//!   the client scrolls the least distance that shows it again.
//! - **Stops.** When the followed process ends, or is no longer shown (a
//!   filter hid it, or the row limit left it out), follow turns itself off
//!   and the status says which and why. It never moves to another row.
//! - **A selection follow overrode.** A promotion replaces a client's local
//!   selection. A `SELECTION_CHANGED` whose `observed_revision` predates the
//!   last promotion was made before the client applied that promotion, so the
//!   promotion replaced it on the client. When follow is on, the server then
//!   promotes the user's row, so the user's later choice wins.
//! - **Off.** No sample ever promotes anything. Only an explicit search does,
//!   as before (PX-011).

use crate::bounded;
use crate::projection::Row;
use crate::source::Observed;
use srui_sdk::ItemId;

pub const FOLLOW_LABEL: &str = "Follow selected process";
pub const FOLLOW_DESCRIPTION: &str =
    "When on, the server keeps the selected process selected and scrolls the table to it whenever a sample, a sort or a filter moves it to another row. Scrolling yourself never changes the selection or turns follow off: the view stays where you put it until the process moves. Selecting another row follows that row. Follow stops, and says why, when the process ends or is no longer shown; a new process that reuses its PID is never followed.";
/// Follow is off: no sample moves the viewport.
pub const OFF_STATUS: &str =
    "Follow: off · the selection stays on its process and the table stays where you scroll it";
pub const UNCOLLECTED_FOLLOW_STATUS: &str = "Follow disabled: process collection not started";
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
struct Followed {
    /// The followed instance's row as last published.
    row: Row,
    /// The row index the client was last shown it at, by a reveal or by the
    /// user's own selection; `None` reveals it on the next plan.
    shown_at: Option<usize>,
}

#[derive(Debug, Clone)]
enum State {
    Off,
    /// Off, because follow was refused or stopped itself: the reason.
    Stopped(String),
    On(Box<Followed>),
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
        pid_text(&row.key.pid),
        bounded(row.display_name.as_str(), MAX_NAME_ECHO_BYTES)
    )
}

fn position(rows: &[Row], item: ItemId) -> Option<usize> {
    rows.iter().position(|row| row.item_id == item)
}

impl Follow {
    /// The followed process's item ID, while follow is on.
    pub fn following(&self) -> Option<ItemId> {
        match &self.state {
            State::On(followed) => Some(followed.row.item_id),
            _ => None,
        }
    }

    /// Turns follow on for `selected`, the inspector's process, and returns
    /// the row to promote now. A selection that is not a shown row is refused
    /// with its reason, and follow stays off.
    pub(crate) fn turn_on(
        &mut self,
        selected: Option<&Row>,
        rows: &[Row],
        unfiltered: &[Row],
    ) -> Option<Row> {
        let Some(selected) = selected else {
            self.state = State::Stopped(
                "Follow: off · nothing is selected; select a row, then turn follow on".into(),
            );
            return None;
        };
        if let Some(index) = position(rows, selected.item_id) {
            let row = rows[index].clone();
            self.state = State::On(Box::new(Followed {
                row: row.clone(),
                shown_at: Some(index),
            }));
            return Some(row);
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
        None
    }

    pub(crate) fn turn_off(&mut self) {
        self.state = State::Off;
    }

    /// Keeps follow on the rows a client will hold, `rows`, out of the
    /// server-side collection `unfiltered`. Returns the row to promote when
    /// the followed process changed row; stops follow, never retargets it,
    /// when the process is not among `rows`.
    pub(crate) fn track(&mut self, rows: &[Row], unfiltered: &[Row]) -> Option<Row> {
        let State::On(followed) = &mut self.state else {
            return None;
        };
        let item = followed.row.item_id;
        if let Some(index) = position(rows, item) {
            followed.row = rows[index].clone();
            if followed.shown_at == Some(index) {
                return None;
            }
            followed.shown_at = Some(index);
            return Some(followed.row.clone());
        }
        let why = match position(unfiltered, item) {
            Some(_) => format!(
                "Follow stopped: {} is no longer shown (filtered out or beyond the row limit); select a row and turn follow on again",
                who(&followed.row)
            ),
            None => format!(
                "Follow stopped: {} has ended · a new process that reuses its PID is not followed; select a row and turn follow on again",
                who(&followed.row)
            ),
        };
        self.state = State::Stopped(why);
        None
    }

    /// Applies a selection while follow is on: `selected` is the shown row it
    /// resolved to, or `None` when it named no shown row. Another row becomes
    /// the followed process; the row is promoted (returned) only when
    /// `overridden`, that is when a later promotion replaced it on the client.
    pub(crate) fn selected(
        &mut self,
        selected: Option<&Row>,
        rows: &[Row],
        overridden: bool,
    ) -> Option<Row> {
        let State::On(followed) = &mut self.state else {
            return None;
        };
        let Some(row) = selected else {
            self.state = State::Stopped(
                "Follow stopped: the selection named no row this server shows; select a row and turn follow on again"
                    .into(),
            );
            return None;
        };
        if row.item_id == followed.row.item_id {
            // The client already shows the followed row selected.
            return None;
        }
        followed.row = row.clone();
        followed.shown_at = position(rows, row.item_id);
        overridden.then(|| row.clone())
    }

    /// Forgets where the client was last shown the followed process, after a
    /// commit failed, so the next plan reveals it again.
    pub(crate) fn forget_shown(&mut self) {
        if let State::On(followed) = &mut self.state {
            followed.shown_at = None;
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
                let at = position(rows, followed.row.item_id)
                    .map(|index| format!(" · row {} of {}", index + 1, rows.len()))
                    .unwrap_or_default();
                FollowView {
                    checked: true,
                    status: bounded(
                        &format!(
                            "Following {}{at} · the table scrolls to it whenever it moves to another row; scrolling yourself never changes the selection or stops follow",
                            who(&followed.row)
                        ),
                        MAX_FOLLOW_STATUS_BYTES,
                    ),
                }
            }
        }
    }
}
