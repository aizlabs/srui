//! The sampling interval, the Pause view / Resume view action and manual
//! refresh (PX-015; design §§12.2, 18, 20.4).
//!
//! Four rules shape this module:
//!
//! * **Pausing freezes this app's publication, not the runtime.** A pause is
//!   a server-side state of the published view: from the revision it names,
//!   the refresh loop takes no sample and no row, cell or summary figure
//!   changes. Core transaction application and the transport are untouched:
//!   every control keeps committing its own transactions, a client keeps
//!   receiving them, and a fresh client attaches to the frozen view like to
//!   any other (§12.2: commits are state-consistency boundaries, not frames).
//! * **Collection stops while paused.** This app is a read-only observer, and a
//!   sample nobody is shown is host work for nothing: resuming takes one new
//!   sample and publishes it as one current view, so no missed sample is ever
//!   replayed. The paused status says so in words ("collection stopped").
//! * **One timer, owned by one loop.** The refresh loop alone owns the ticker.
//!   A handler never starts a timer: it records a request here and wakes the
//!   loop through one [`Notify`], whose single stored permit coalesces any
//!   number of presses into one wake. An interval change replaces the loop's
//!   ticker, so two never run, and the next periodic sample is one new
//!   interval after the change: no burst, and no drift from the old phase.
//! * **A sample is applied only to the view it was taken for.** Every pause,
//!   every resume and every change of "Show user threads" (PX-019) advances
//!   an epoch, and a sample whose epoch is no longer current is discarded, so
//!   a scan that was running when the view was paused (or paused and resumed)
//!   can never change the frozen rows or stand in for the sample Resume view
//!   asked for, and one taken under the other thread setting is never
//!   applied. A thread-setting change on a live view asks for one new sample
//!   at once, so the sample it discards is always replaced.
use crate::bounded;
use crate::metric::format_utc;
use crate::refresh::{MAX_REFRESH_INTERVAL, MIN_REFRESH_INTERVAL};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;

/// The Pause view / Resume view button's label while the view is live.
pub const PAUSE_LABEL: &str = "Pause view";
/// The same button's label while the view is paused.
pub const RESUME_LABEL: &str = "Resume view";
/// The manual refresh button's label.
pub const REFRESH_LABEL: &str = "Refresh now";
/// The interval field's label.
pub const INTERVAL_LABEL: &str = "Refresh interval (ms)";
/// The interval field's placeholder.
pub const INTERVAL_PLACEHOLDER: &str = "50 to 300000 ms";
pub const INTERVAL_DESCRIPTION: &str =
    "Whole milliseconds between samples, from 50 to 300000. Apply interval checks the value on the server; a value it refuses is explained on the sampling status and the current interval is kept.";
/// The button that applies the interval field's value.
pub const INTERVAL_APPLY_LABEL: &str = "Apply interval";
/// The sampling status of the shell that collects nothing.
pub const UNCOLLECTED_SAMPLING_STATUS: &str = "Sampling disabled: process collection not started";
/// The most interval text the server keeps; anything longer is refused.
pub const MAX_INTERVAL_INPUT_BYTES: usize = 32;
/// The most bytes the sampling status holds before it is cut and marked.
pub const MAX_SAMPLING_STATUS_BYTES: usize = 512;
/// What the data-freshness line starts with while the view is paused (PX-007).
pub const PAUSED_FRESHNESS: &str = "Paused, not live · ";
/// What it starts with between Resume view and the sample it asked for.
pub const RESUMING_FRESHNESS: &str = "Resuming, not yet refreshed · ";

/// Why an interval was refused. The previous interval is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalError {
    Empty,
    TooLong,
    NotANumber,
    OutOfRange,
}

impl std::fmt::Display for IntervalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let range = format!(
            "{} to {} ms",
            MIN_REFRESH_INTERVAL.as_millis(),
            MAX_REFRESH_INTERVAL.as_millis()
        );
        match self {
            Self::Empty => write!(formatter, "no interval entered; enter {range}"),
            Self::TooLong => write!(
                formatter,
                "longer than {MAX_INTERVAL_INPUT_BYTES} bytes; enter {range}"
            ),
            Self::NotANumber => {
                write!(
                    formatter,
                    "not a whole number of milliseconds; enter {range}"
                )
            }
            Self::OutOfRange => write!(formatter, "outside {range}"),
        }
    }
}

impl std::error::Error for IntervalError {}

/// The interval `text` names: whole milliseconds, ASCII digits only, inside
/// [`MIN_REFRESH_INTERVAL`]..=[`MAX_REFRESH_INTERVAL`]. Surrounding spaces are
/// ignored; a sign, a fraction, a unit or any other character is refused,
/// never rounded or clamped into something else. The command line and the
/// interval field share this one validator.
pub fn parse_interval(text: &str) -> Result<Duration, IntervalError> {
    if text.len() > MAX_INTERVAL_INPUT_BYTES {
        return Err(IntervalError::TooLong);
    }
    let digits = text.trim();
    if digits.is_empty() {
        return Err(IntervalError::Empty);
    }
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(IntervalError::NotANumber);
    }
    // All digits, so the only parse failure left is overflow: out of range.
    let milliseconds: u64 = digits.parse().map_err(|_| IntervalError::OutOfRange)?;
    let interval = Duration::from_millis(milliseconds);
    if (MIN_REFRESH_INTERVAL..=MAX_REFRESH_INTERVAL).contains(&interval) {
        Ok(interval)
    } else {
        Err(IntervalError::OutOfRange)
    }
}

/// Where a paused view was frozen: the server revision whose rows, cells and
/// summary it keeps showing, and when, on the server's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frozen {
    /// The session revision current when the pause was applied. Every row,
    /// cell and summary figure the view shows is the one that revision held.
    pub revision: u64,
    pub paused_at: SystemTime,
}

/// What the refresh loop is asked to do, read and cleared under the view's
/// lock on every wake and every tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplingRequest {
    pub interval: Duration,
    pub paused: bool,
    /// The view's epoch: a sample taken now is applied only if this is still
    /// the epoch when it returns.
    pub epoch: u64,
    /// Take one sample now (a Refresh now press, a Resume view, or a change of
    /// "Show user threads", PX-019), however many requests asked for it.
    pub sample_now: bool,
    /// Restart the ticker: the interval changed, or the view was resumed.
    pub retime: bool,
}

/// The sampling controls as a client holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingView {
    pub status: String,
    pub pause_label: String,
    pub refresh_enabled: bool,
}

impl SamplingView {
    /// The controls of the shell that collects nothing.
    pub fn uncollected() -> Self {
        Self {
            status: UNCOLLECTED_SAMPLING_STATUS.to_string(),
            pause_label: PAUSE_LABEL.to_string(),
            refresh_enabled: false,
        }
    }
}

/// The server-side sampling state of one published view.
pub struct Sampling {
    interval: Duration,
    frozen: Option<Frozen>,
    /// Resume view was applied and its sample has not been published yet.
    resuming: bool,
    epoch: u64,
    sample_now: bool,
    retime: bool,
    /// The interval field's text as the server keeps it, at most
    /// [`MAX_INTERVAL_INPUT_BYTES`].
    input: String,
    /// Whether the field's last text was longer than the server keeps.
    input_too_long: bool,
    /// Why the last interval was refused, until another one is accepted.
    rejection: Option<String>,
    /// Samples discarded because they were taken for an earlier epoch or
    /// returned while the view was paused.
    discarded: u64,
    wake: Arc<Notify>,
}

impl Sampling {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            frozen: None,
            resuming: false,
            epoch: 0,
            sample_now: false,
            retime: false,
            input: interval.as_millis().to_string(),
            input_too_long: false,
            rejection: None,
            discarded: 0,
            wake: Arc::new(Notify::new()),
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Where the view is frozen, if it is paused. Process-changing actions
    /// (none exist yet) must refuse a target while this is `Some`: the rows
    /// shown are historical data, not the host's current processes.
    pub fn frozen(&self) -> Option<Frozen> {
        self.frozen
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn discarded(&self) -> u64 {
        self.discarded
    }

    /// The interval field's text as the server keeps it.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The loop's wake-up. Shared, never a second timer.
    pub fn wake_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.wake)
    }

    fn wake(&self) {
        self.wake.notify_one();
    }

    /// Reads and clears the loop's request.
    pub fn take_request(&mut self) -> SamplingRequest {
        let request = SamplingRequest {
            interval: self.interval,
            paused: self.frozen.is_some(),
            epoch: self.epoch,
            sample_now: self.sample_now && self.frozen.is_none(),
            retime: self.retime,
        };
        self.sample_now = false;
        self.retime = false;
        request
    }

    /// Whether a sample taken under `epoch` may be published now.
    pub(crate) fn accepts(&mut self, epoch: u64) -> bool {
        let accepted = self.frozen.is_none() && epoch == self.epoch;
        if !accepted {
            self.discarded += 1;
        }
        accepted
    }

    /// A sample is being published: the view is live again.
    pub(crate) fn published_sample(&mut self) {
        self.resuming = false;
    }

    /// Records a sample refused because the view is frozen.
    pub(crate) fn discard(&mut self) {
        self.discarded += 1;
    }

    pub(crate) fn pause(&mut self, frozen: Frozen) {
        self.frozen = Some(frozen);
        self.resuming = false;
        self.sample_now = false;
        self.epoch += 1;
        self.wake();
    }

    pub(crate) fn resume(&mut self) {
        self.frozen = None;
        self.resuming = true;
        self.sample_now = true;
        self.retime = true;
        self.epoch += 1;
        self.wake();
    }

    /// Moves to a new epoch, so a sample already being taken is discarded
    /// when it returns: it was taken under a setting that no longer holds
    /// (PX-019's "Show user threads"). Unless the view is paused, one new
    /// sample is asked for at once, so a discarded sample — Resume view's
    /// own, or a Refresh now press — is always replaced; a paused view takes
    /// none until it is resumed.
    pub(crate) fn invalidate(&mut self) {
        self.epoch += 1;
        if self.frozen.is_none() {
            self.sample_now = true;
            self.wake();
        }
    }

    /// One Refresh now press: one sample, coalesced with any other press the
    /// loop has not taken yet. Refused while paused.
    pub(crate) fn refresh_now(&mut self) -> bool {
        if self.frozen.is_some() {
            return false;
        }
        self.sample_now = true;
        self.wake();
        true
    }

    /// Keeps the interval field's `text`, at most [`MAX_INTERVAL_INPUT_BYTES`]
    /// of it, and returns what is kept. A refusal explained text the field
    /// no longer holds, so it is cleared.
    pub(crate) fn set_input(&mut self, text: &str) -> &str {
        self.rejection = None;
        self.input_too_long = text.len() > MAX_INTERVAL_INPUT_BYTES;
        self.input = if self.input_too_long {
            let mut end = MAX_INTERVAL_INPUT_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text[..end].to_string()
        } else {
            text.to_string()
        };
        &self.input
    }

    /// Applies the interval field's text: the new interval, or the reason it
    /// was refused, with the previous interval kept.
    pub(crate) fn apply_input(&mut self) -> Result<Duration, IntervalError> {
        let parsed = if self.input_too_long {
            Err(IntervalError::TooLong)
        } else {
            parse_interval(&self.input)
        };
        match parsed {
            Ok(interval) => {
                self.rejection = None;
                self.set_interval(interval);
                Ok(interval)
            }
            Err(error) => {
                self.rejection = Some(format!(
                    "interval {:?} refused: {error}; keeping {} ms",
                    self.input,
                    self.interval.as_millis()
                ));
                Err(error)
            }
        }
    }

    /// Makes `interval` the sampling interval, waking the loop to restart its
    /// one ticker if it changed. Out-of-range values are clamped: only a
    /// caller that skipped [`parse_interval`] can pass one.
    pub(crate) fn set_interval(&mut self, interval: Duration) {
        let interval = interval.clamp(MIN_REFRESH_INTERVAL, MAX_REFRESH_INTERVAL);
        if interval != self.interval {
            self.interval = interval;
            self.retime = true;
            self.wake();
        }
    }

    /// The sampling status line.
    pub fn status(&self) -> String {
        let every = self.interval.as_millis();
        let mut status = match (&self.frozen, self.resuming) {
            (Some(frozen), _) => format!(
                "Paused at {} (server clock) · rows as of revision {} · collection stopped: \
                 table and summary frozen; filters, sort, columns, search and the inspector \
                 act on the frozen rows · Resume view publishes a current sample, then every \
                 {every} ms",
                format_utc(frozen.paused_at)
                    .unwrap_or_else(|| "before 1970-01-01 00:00:00 UTC".to_string()),
                frozen.revision
            ),
            (None, true) => format!("Resuming · sampling now, then every {every} ms"),
            (None, false) => format!("Live · sampling every {every} ms"),
        };
        if let Some(rejection) = &self.rejection {
            let _ = write!(status, " · {rejection}");
        }
        bounded(&status, MAX_SAMPLING_STATUS_BYTES)
    }

    pub fn view(&self) -> SamplingView {
        SamplingView {
            status: self.status(),
            pause_label: if self.frozen.is_some() {
                RESUME_LABEL
            } else {
                PAUSE_LABEL
            }
            .to_string(),
            refresh_enabled: self.frozen.is_none(),
        }
    }

    /// The prefix the data-freshness line carries now, if any.
    pub(crate) fn freshness_prefix(&self) -> Option<&'static str> {
        if self.frozen.is_some() {
            Some(PAUSED_FRESHNESS)
        } else if self.resuming {
            Some(RESUMING_FRESHNESS)
        } else {
            None
        }
    }
}

/// `text` with any sampling prefix replaced by `prefix`: the data-freshness
/// line as the current sampling state labels it. A line already cut keeps
/// its cut; the next published sample recomputes the whole line.
pub(crate) fn relabel_freshness(text: &str, prefix: Option<&str>) -> String {
    let base = text
        .strip_prefix(PAUSED_FRESHNESS)
        .or_else(|| text.strip_prefix(RESUMING_FRESHNESS))
        .unwrap_or(text);
    match prefix {
        Some(prefix) => format!("{prefix}{base}"),
        None => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interval_is_whole_milliseconds_inside_the_range_and_nothing_else() {
        assert_eq!(parse_interval("50"), Ok(Duration::from_millis(50)));
        assert_eq!(parse_interval(" 1500 "), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_interval("300000"), Ok(Duration::from_secs(300)));
        for (text, error) in [
            ("", IntervalError::Empty),
            ("   ", IntervalError::Empty),
            ("49", IntervalError::OutOfRange),
            ("0", IntervalError::OutOfRange),
            ("300001", IntervalError::OutOfRange),
            ("99999999999999999999999", IntervalError::OutOfRange),
            ("+500", IntervalError::NotANumber),
            ("-500", IntervalError::NotANumber),
            ("0.5", IntervalError::NotANumber),
            ("500ms", IntervalError::NotANumber),
            ("1e3", IntervalError::NotANumber),
            ("５００", IntervalError::NotANumber),
            (
                &"9".repeat(MAX_INTERVAL_INPUT_BYTES + 1),
                IntervalError::TooLong,
            ),
        ] {
            assert_eq!(parse_interval(text), Err(error), "{text:?}");
        }
    }

    #[test]
    fn a_refused_interval_is_explained_and_the_previous_one_kept() {
        let mut sampling = Sampling::new(Duration::from_millis(1000));
        let _ = sampling.take_request();
        sampling.set_input("fast");
        assert_eq!(sampling.apply_input(), Err(IntervalError::NotANumber));
        assert_eq!(sampling.interval(), Duration::from_millis(1000));
        assert!(
            !sampling.take_request().retime,
            "a refusal restarts no timer"
        );
        let status = sampling.status();
        assert!(
            status.starts_with("Live · sampling every 1000 ms · interval \"fast\" refused: "),
            "{status}"
        );
        sampling.set_input(&"7".repeat(100));
        assert_eq!(
            sampling.status(),
            "Live · sampling every 1000 ms",
            "an edit clears the refusal of the text it replaced"
        );
        assert_eq!(sampling.input().len(), MAX_INTERVAL_INPUT_BYTES);
        assert_eq!(sampling.apply_input(), Err(IntervalError::TooLong));
        sampling.set_input("250");
        assert_eq!(sampling.apply_input(), Ok(Duration::from_millis(250)));
        assert_eq!(sampling.status(), "Live · sampling every 250 ms");
        assert!(sampling.take_request().retime);
        // The same interval again restarts nothing.
        assert_eq!(sampling.apply_input(), Ok(Duration::from_millis(250)));
        assert!(!sampling.take_request().retime);
    }

    #[test]
    fn presses_coalesce_and_a_pause_or_resume_retires_every_earlier_sample() {
        let mut sampling = Sampling::new(Duration::from_millis(1000));
        for _ in 0..5 {
            assert!(sampling.refresh_now());
        }
        let request = sampling.take_request();
        assert!(request.sample_now);
        assert!(
            !sampling.take_request().sample_now,
            "five presses, one sample"
        );
        let before = request.epoch;
        sampling.pause(Frozen {
            revision: 7,
            paused_at: SystemTime::UNIX_EPOCH,
        });
        assert!(!sampling.refresh_now(), "no manual sample while paused");
        let paused = sampling.take_request();
        assert!(paused.paused && !paused.sample_now);
        assert!(!sampling.accepts(before), "a sample from before the pause");
        assert!(!sampling.accepts(paused.epoch), "no sample while paused");
        assert!(sampling
            .status()
            .starts_with("Paused at 1970-01-01 00:00:00 UTC (server clock) · rows as of revision 7 · collection stopped"));
        assert_eq!(sampling.view().pause_label, RESUME_LABEL);
        assert!(!sampling.view().refresh_enabled);
        sampling.resume();
        let resumed = sampling.take_request();
        assert!(resumed.sample_now && resumed.retime && !resumed.paused);
        assert!(
            !sampling.accepts(paused.epoch),
            "a sample from the paused epoch"
        );
        assert!(sampling.accepts(resumed.epoch));
        assert_eq!(sampling.discarded(), 3);
        assert_eq!(
            sampling.status(),
            "Resuming · sampling now, then every 1000 ms"
        );
        sampling.published_sample();
        assert_eq!(sampling.status(), "Live · sampling every 1000 ms");
    }

    #[test]
    fn the_freshness_line_is_relabelled_without_stacking_prefixes() {
        let live = "Last successful sample: x";
        let paused = relabel_freshness(live, Some(PAUSED_FRESHNESS));
        assert_eq!(paused, "Paused, not live · Last successful sample: x");
        assert_eq!(relabel_freshness(&paused, Some(PAUSED_FRESHNESS)), paused);
        let resuming = relabel_freshness(&paused, Some(RESUMING_FRESHNESS));
        assert_eq!(
            resuming,
            "Resuming, not yet refreshed · Last successful sample: x"
        );
        assert_eq!(relabel_freshness(&resuming, None), live);
    }

    #[test]
    fn the_widest_status_is_bounded() {
        let mut sampling = Sampling::new(MAX_REFRESH_INTERVAL);
        sampling.pause(Frozen {
            revision: u64::MAX,
            paused_at: SystemTime::UNIX_EPOCH + Duration::from_secs(253_402_300_799),
        });
        sampling.set_input(&"\u{20000}".repeat(MAX_INTERVAL_INPUT_BYTES / 4));
        let _ = sampling.apply_input();
        let status = sampling.status();
        assert!(status.len() <= MAX_SAMPLING_STATUS_BYTES + '…'.len_utf8());
        assert!(status.starts_with("Paused at "));
    }
}
