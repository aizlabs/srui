//! Metric definitions: what one published number is, where it was read, and how
//! it must be read back (design §§6.2, 22, 29; PX-005 resident memory, PX-006
//! sampled CPU usage, PX-007 system-wide figures and the sample time).
//!
//! Interpretation is server-side. A metric is stored as the exact integer the
//! kernel reported, in the unit its definition names, and this module is the one
//! place that turns it into the text a row carries. The client renders that text
//! and never converts a unit, scales a byte count or decides what an absent
//! value means — so there is no process-specific client policy to keep in step,
//! and two clients cannot disagree about what a number says.
//!
//! Precision is the reason the conversion lives here rather than in a format
//! string. A byte count is a `u64`: at the scale of a modern host, `f64` cannot
//! represent every one of them, and a float that rounds *up* publishes a value
//! larger than the one that was measured. Every multiple below is a power of
//! 1024, so the whole part is a shift, the fraction is an integer remainder, and
//! no step of the conversion leaves the integers.
use crate::source::{CpuInterval, CpuUsage, MissingReason, Observed};
use std::time::{SystemTime, UNIX_EPOCH};

/// What one published metric is: the value's unit, where the number came from,
/// and how to read it. A definition is documentation that travels with the code
/// that produces the number, so a row's meaning cannot drift from its source.
///
/// This is deliberately not a registry: PX-042 owns registering metrics,
/// scheduling them and describing their availability. This ticket adds one
/// metric and states what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricDefinition {
    /// Stable identifier, never a display string.
    pub id: &'static str,
    /// Column heading this metric is published under.
    pub label: &'static str,
    /// Unit of the stored value, which is always an exact integer.
    pub unit: &'static str,
    /// Exactly which interface the number is read from.
    pub source: &'static str,
    /// What the number does and does not account for.
    pub interpretation: &'static str,
}

/// Resident set size: the pages a process has in real memory now.
pub const RESIDENT_MEMORY: MetricDefinition = MetricDefinition {
    id: "process.resident_memory",
    label: "Resident",
    unit: "bytes",
    source: "Linux /proc/<pid>/statm field 2 (resident), in pages, multiplied by the page size \
            the kernel reports through AT_PAGESZ in /proc/self/auxv of the scanned mount, and read \
            after the stat line that identifies the process: through the same /proc/<pid> \
            directory handle where the reader's own /proc can pin one, and by name where it \
            cannot (K1)",
    interpretation: "Pages this process has in real memory at sample time. Shared pages are \
                     counted in full for every process that maps them, so these values do not sum \
                     to the memory a host has in use; pages that are swapped out or were never \
                     faulted in are not counted at all. Exact from Linux 6.16 (and the stable \
                     kernels carrying the same fix: 6.15.7, 6.12.39, 6.6.99), where statm sums \
                     the kernel's per-CPU counters. From 6.2 up to that fix it is the same \
                     approximate count as /proc/<pid>/stat field 24: each of the file, anonymous \
                     and shared-memory counters it adds up may be off, in either direction, by \
                     less than max(32, 2 x online CPUs) pages per online CPU, so a just-started \
                     or very small process can read 0 B. Field 24 stays approximate on every \
                     kernel since 6.2 and is not used. Before 6.2 statm and stat read one counter \
                     that is approximate too: each thread adds its cached changes in only after \
                     more than 64 page-fault events (SPLIT_RSS_COUNTING). Proportional and \
                     shared-page accounting is PX-046, not this metric.",
};

/// CPU time a process used over one sampling interval, as a share of **one**
/// logical CPU (S1's convention: one fully busy logical CPU is 100%, so a
/// multithreaded process may exceed 100%). The heading states the convention,
/// because the number alone cannot: 150% is one and a half CPUs here, never
/// "one and a half times the host". A share normalized to the whole host is a
/// later display option, not this metric.
pub const CPU_USAGE: MetricDefinition = MetricDefinition {
    id: "process.cpu_usage",
    label: "CPU (100% = 1 CPU)",
    unit: "tenths of a percent of one logical CPU",
    source: "Linux /proc/<pid>/stat fields 14 (utime) and 15 (stime), in clock ticks, divided by \
            the tick rate the kernel reports through AT_CLKTCK in /proc/self/auxv of the scanned \
            mount, over the interval between two scans of the same process instance measured on \
            a monotonic clock (K1)",
    interpretation: "The user plus system CPU time this process instance was scheduled for \
                     between two scans, divided by the time that passed between them. 100% is \
                     one fully used logical CPU, so a multithreaded process may exceed 100%. \
                     Waited-for children's time (cutime, cstime) is not included. A value needs \
                     two samples of the same instance: the first sample of a process, of a \
                     replacement under a reused PID, after a counter went backwards, or across \
                     an interval that did not advance is published as warming up or unavailable, \
                     never as zero and never as a spike. The interval is measured on a monotonic \
                     clock, so a wall-clock change cannot stretch or shrink it.",
};

/// Published while a process instance has only one sample: there is no interval
/// yet to divide by. It is a state, not a quantity, so it carries no digit.
pub const CPU_WARMING_UP: &str = "Warming up";

/// The most logical CPUs a measured value may imply before it is refused as a
/// counter discontinuity rather than published. Far above any real host
/// (Linux's own `NR_CPUS` ceiling is 8192), and it is what bounds the widest
/// CPU cell at `6553600.0%`, inside [`MAX_CELL_BYTES`].
pub const MAX_PLAUSIBLE_LOGICAL_CPUS: u64 = 65_536;

/// A measured interval as tenths of a percent of one logical CPU, truncated, or
/// `None` when the interval cannot be divided by or implies more CPUs than
/// [`MAX_PLAUSIBLE_LOGICAL_CPUS`].
///
/// `ticks / ticks_per_second` is CPU seconds; dividing by the elapsed seconds and
/// scaling by 1000 gives tenths of a percent. Every step is integer arithmetic in
/// `u128`, checked, and truncating — like [`format_iec_bytes`], a value is never
/// displayed as larger than it was measured.
pub fn cpu_tenths_of_percent(interval: &CpuInterval) -> Option<u64> {
    let elapsed = interval.elapsed.as_nanos();
    if elapsed == 0 || interval.ticks_per_second == 0 {
        return None;
    }
    let numerator = u128::from(interval.ticks).checked_mul(1_000 * 1_000_000_000)?;
    let denominator = u128::from(interval.ticks_per_second).checked_mul(elapsed)?;
    let tenths = numerator / denominator;
    if tenths > u128::from(MAX_PLAUSIBLE_LOGICAL_CPUS) * 1_000 {
        return None;
    }
    u64::try_from(tenths).ok()
}

/// Tenths of a percent as published text: `12.3%`, `0.0%`, `250.0%`.
pub fn format_cpu_tenths(tenths: u64) -> String {
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// The published text of a CPU usage sample: a measured share, the warming-up
/// state, or why it could not be measured. Neither of the last two is ever
/// published as `0.0%`, and a *measured* zero is never published as either.
pub fn cpu_cell(usage: &CpuUsage) -> String {
    match usage {
        CpuUsage::Measured(interval) => match cpu_tenths_of_percent(interval) {
            Some(tenths) => format_cpu_tenths(tenths),
            None => missing_text(MissingReason::Unavailable).to_string(),
        },
        CpuUsage::WarmingUp => CPU_WARMING_UP.to_string(),
        CpuUsage::Missing(reason) => missing_text(*reason).to_string(),
    }
}

/// IEC binary multiples, in ascending order. Each is 1024 times the one before,
/// so converting between them is a shift (§22: a published unit is never
/// ambiguous — `MiB` is 1024², never 10⁶).
const IEC_UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// The longest text [`bytes_cell`] or [`cpu_cell`] can publish, in encoded bytes.
///
/// It is [`missing_text`]'s longest wording, not a number: the widest value the
/// byte formatter can emit is `"1023.9 PiB"` at ten bytes, the widest CPU value
/// `"6553600.0%"` at ten, and [`CPU_WARMING_UP`] is ten. The bound is asserted
/// against all of them by `no_published_cell_is_wider_than_the_reserved_bound`
/// and `no_cpu_cell_is_wider_than_the_reserved_bound`, and
/// `crate::refresh` charges a row's cells by their real encoded length, so this
/// is what the widest row in its frame-ceiling proof carries.
pub const MAX_CELL_BYTES: usize = 11;

/// `bytes` in the largest IEC multiple whose whole part is at least one, with
/// one fractional digit above `B`.
///
/// The fraction is **truncated**, never rounded: a value is never displayed as
/// larger than it is, and a count just below a multiple reads `1023.9 KiB`
/// rather than the `1024.0 KiB` that rounding would produce — a unit boundary
/// the value never reached. Every operation is integer: the whole part is a
/// shift, and the fraction is an exact remainder scaled by ten before the shift
/// back, which cannot overflow because a remainder is below `1 << 60` and
/// `(1 << 60) * 10` fits in a `u64`.
pub fn format_iec_bytes(bytes: u64) -> String {
    let mut exponent = 0u32;
    while (exponent as usize) + 1 < IEC_UNITS.len() && bytes >> ((exponent + 1) * 10) > 0 {
        exponent += 1;
    }
    let unit = IEC_UNITS[exponent as usize];
    if exponent == 0 {
        // Below a kibibyte the exact count is shorter than any fraction of one.
        return format!("{bytes} {unit}");
    }
    let shift = exponent * 10;
    let whole = bytes >> shift;
    let remainder = bytes - (whole << shift);
    let tenths = (remainder * 10) >> shift;
    format!("{whole}.{tenths} {unit}")
}

/// The published text of a byte-valued metric: its value, or why this scan could
/// not read it.
///
/// A metric the scan could not read is never published as zero, and a *known*
/// zero is never published as unavailable: a kernel thread with no resident
/// pages is a fact, and both states have to survive the trip to the row.
pub fn bytes_cell(observed: &Observed<u64>) -> String {
    match observed {
        Observed::Known(bytes) => format_iec_bytes(*bytes),
        Observed::Missing(reason) => missing_text(*reason).to_string(),
    }
}

/// The host's overall CPU usage (PX-007): a share of **all** logical CPUs, so
/// 100% is every logical CPU busy for the whole interval — never the process
/// column's convention, where 100% is one CPU ([`CPU_USAGE`]). The label names
/// that denominator, because the number alone cannot.
pub const SYSTEM_CPU: MetricDefinition = MetricDefinition {
    id: "system.cpu_busy",
    label: "Overall CPU (100% = all logical CPUs)",
    unit: "tenths of a percent of the time of all logical CPUs",
    source: "Linux /proc/stat, first line (cpu): fields user, nice, system, idle, iowait, irq, \
             softirq and steal, in USER_HZ clock ticks summed over every CPU, read through the \
             scanned root and differenced between two reads of the same boot; the cpuN lines \
             that follow it are counted as the logical CPUs online (K1)",
    interpretation: "Busy is user + nice + system + irq + softirq + steal and idle is idle + \
                     iowait; the share is the busy delta over the busy plus idle delta, so 100% \
                     means every logical CPU was busy for the whole interval, where the process \
                     column's 100% is one CPU. guest and guest_nice are already counted in user \
                     and nice and are not added again, and fields after steal are not read. \
                     iowait may decrease between two reads (K1), so idle and iowait are \
                     differenced as one sum. The share is always measured between two \
                     consecutive successful reads: a first read, and the first read after one \
                     that failed, is warming up, so an outage is never averaged into a current \
                     figure; counters that went backwards, did not advance, or were read over a \
                     different number of cpuN lines leave no interval and are published as \
                     unavailable, never as zero and never as a spike.",
};

/// Memory in use on the host (PX-007).
pub const MEMORY_USED: MetricDefinition = MetricDefinition {
    id: "system.memory_used",
    label: "Memory",
    unit: "bytes",
    source: "Linux /proc/meminfo MemTotal and MemAvailable, in kB of 1024 bytes, read through \
             the scanned root (K1)",
    interpretation: "Used is MemTotal - MemAvailable: the memory the kernel does not estimate \
                     as available for starting new applications without swapping. The \
                     percentage is used / MemTotal, truncated. A missing or malformed field, a \
                     MemTotal of 0, or a MemAvailable above MemTotal is unavailable, never \
                     zero.",
};

/// Swap space in use on the host (PX-007).
pub const SWAP_USED: MetricDefinition = MetricDefinition {
    id: "system.swap_used",
    label: "Swap",
    unit: "bytes",
    source: "Linux /proc/meminfo SwapTotal and SwapFree, in kB of 1024 bytes, read through the \
             scanned root (K1)",
    interpretation: "Used is SwapTotal - SwapFree, and the percentage is used / SwapTotal, \
                     truncated. A SwapTotal of 0 is a host with no swap space and is published \
                     as such: never 0% of 0 and never a division. A SwapFree above SwapTotal \
                     is unavailable.",
};

/// Time since the host booted (PX-007).
pub const UPTIME: MetricDefinition = MetricDefinition {
    id: "system.uptime",
    label: "Uptime",
    unit: "seconds",
    source: "Linux /proc/uptime, first field: seconds since boot on the boot-time clock, so \
             time spent suspended counts, read through the scanned root (K1)",
    interpretation: "Published truncated to whole minutes, never rounded up.",
};

/// The host's load averages (PX-007).
pub const LOAD_AVERAGE: MetricDefinition = MetricDefinition {
    id: "system.load_average",
    label: "Load average (1, 5, 15 min)",
    unit: "hundredths of a task",
    source: "Linux /proc/loadavg, first three fields, each printed by the kernel with exactly \
             two decimals, read through the scanned root (K1)",
    interpretation: "Exponentially damped averages, over 1, 5 and 15 minutes, of the tasks \
                     that are runnable or in uninterruptible sleep. Not a percentage and not \
                     divided by the number of CPUs; published exactly as the kernel printed \
                     it.",
};

/// How many processes the last successful scan listed (PX-007), with the only
/// scope srtop can state for that count: what this reader can see in the
/// scanned root.
pub const PROCESS_COUNT: MetricDefinition = MetricDefinition {
    id: "system.process_count",
    label: "Processes visible to this reader",
    unit: "processes",
    source: "The scan's own record list: the numeric directories of the scanned root that this \
             reader can see, one per process (thread group), never one per thread",
    interpretation: "The records the last successful scan listed and read, with the records \
                     it could not read and the entries beyond its record limit counted \
                     separately, in the status line's own words; a process that ended during \
                     the scan is not counted. The count is bounded by what this reader can see, \
                     not by the host: its PID namespace hides every process outside it, and a \
                     procfs mounted with hidepid=invisible (2), as systemd's \
                     ProtectProc=invisible does, or hidepid=ptraceable (4), hides other \
                     users' or unptraceable processes without any error, so a complete scan \
                     is complete over this view only. srtop detects neither, and applies no \
                     filter of its own.",
};

/// When the figures on screen were sampled (PX-007).
pub const SAMPLE_TIME: MetricDefinition = MetricDefinition {
    id: "system.sample_time",
    label: "Last successful sample",
    unit: "seconds since 1970-01-01 00:00:00 UTC",
    source: "The collecting server's wall clock when the scan began; a fixture states its own",
    interpretation: "The time of the last scan whose process list was read, in UTC to the \
                     second. A scan that fails leaves it, and every figure, as they were and \
                     marks a collector error. The client derives no age from it, so the age is \
                     this time against the reader's own clock, and transport loss is the \
                     client's own indicator, never this one.",
};

/// `part` as tenths of a percent of `whole`, truncated, or `None` where that
/// share is not defined: an empty whole, or a part larger than its whole.
///
/// Integer arithmetic in `u128`, so no product overflows and a share is never
/// displayed as larger than it was measured (PX-007).
pub fn share_tenths(part: u64, whole: u64) -> Option<u64> {
    if whole == 0 || part > whole {
        return None;
    }
    u64::try_from(u128::from(part) * 1_000 / u128::from(whole)).ok()
}

/// A count in hundredths, as the kernel prints a load average: `0.52`, `12.00`.
pub fn format_hundredths(hundredths: u64) -> String {
    format!("{}.{:02}", hundredths / 100, hundredths % 100)
}

/// Seconds since boot, truncated to whole minutes: `4 h 05 min`,
/// `1 day, 0 h 00 min`, `3 days, 4 h 05 min`.
pub fn format_uptime(seconds: u64) -> String {
    let minutes = seconds / 60;
    let (days, hours, minutes) = (minutes / 1_440, minutes / 60 % 24, minutes % 60);
    let clock = format!("{hours} h {minutes:02} min");
    match days {
        0 => clock,
        1 => format!("1 day, {clock}"),
        _ => format!("{days} days, {clock}"),
    }
}

/// A wall-clock time in UTC, truncated to the second: `2027-01-15 08:00:00
/// UTC`, or `None` for a time before 1970-01-01 00:00:00 UTC.
///
/// Integer arithmetic only, so every representable time is formatted exactly,
/// and no dependency: the civil date is H. Hinnant's days-to-civil algorithm
/// (proleptic Gregorian calendar, days since 1970-01-01).
pub fn format_utc(time: SystemTime) -> Option<String> {
    let seconds = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        of_day / 3_600,
        of_day / 60 % 60,
        of_day % 60
    ))
}

/// Cell wording for a value this scan could not read.
///
/// Separate from [`MissingReason::describe`], which words a status-line clause
/// (`"permission denied"` reads as a sentence fragment, not as a table cell).
/// Every cell in a row that could not be read is worded by this one function, so
/// a missing PID and a missing metric cannot drift apart.
pub fn missing_text(reason: MissingReason) -> &'static str {
    match reason {
        MissingReason::Unavailable => "Unavailable",
        MissingReason::Denied => "Denied",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iec_multiples_are_exact_at_every_boundary_and_never_round_up() {
        for (bytes, expected) in [
            (0u64, "0 B"),
            (1, "1 B"),
            (1023, "1023 B"),
            (1024, "1.0 KiB"),
            (1025, "1.0 KiB"),
            (1536, "1.5 KiB"),
            // One byte below a mebibyte is not a mebibyte, and not 1024.0 KiB.
            (1_048_575, "1023.9 KiB"),
            (1_048_576, "1.0 MiB"),
            (1_234_567, "1.1 MiB"),
            (1_073_741_823, "1023.9 MiB"),
            (1_073_741_824, "1.0 GiB"),
            (5_368_709_120, "5.0 GiB"),
            (1_099_511_627_776, "1.0 TiB"),
            (1_125_899_906_842_624, "1.0 PiB"),
            // The largest multiple is the last one: a value beyond it keeps
            // growing the whole part rather than inventing a unit.
            (1_152_921_504_606_846_976, "1.0 EiB"),
            (u64::MAX, "15.9 EiB"),
        ] {
            assert_eq!(format_iec_bytes(bytes), expected, "{bytes}");
        }
    }

    /// The case a floating-point conversion gets wrong. `f64` cannot represent
    /// either of these counts, and dividing them by their multiple rounds *up*
    /// to the next whole unit — publishing a value larger than the one measured,
    /// at a unit boundary neither value reached.
    #[test]
    fn a_byte_count_too_large_for_a_float_is_still_published_exactly() {
        for (bytes, expected, float_would_say) in [
            (u64::MAX, "15.9 EiB", "16.0 EiB"),
            (u64::MAX / 2, "7.9 EiB", "8.0 EiB"),
        ] {
            assert_eq!(format_iec_bytes(bytes), expected);
            let float = bytes as f64 / 1_152_921_504_606_846_976f64;
            assert_eq!(format!("{float:.1} EiB"), float_would_say);
            assert_ne!(format_iec_bytes(bytes), float_would_say);
        }
        // Neither is a rounding artifact of the formatter: both counts really
        // are below the whole unit a float reports them as.
        assert!(u128::from(u64::MAX) < 16u128 * 1_152_921_504_606_846_976u128);
        assert!(u128::from(u64::MAX / 2) < 8u128 * 1_152_921_504_606_846_976u128);
    }

    #[test]
    fn a_known_zero_is_published_as_zero_and_an_unread_value_never_is() {
        assert_eq!(bytes_cell(&Observed::Known(0)), "0 B");
        assert_eq!(
            bytes_cell(&Observed::Missing(MissingReason::Unavailable)),
            "Unavailable"
        );
        assert_eq!(
            bytes_cell(&Observed::Missing(MissingReason::Denied)),
            "Denied"
        );
        // A value that could not be read must not read as any quantity at all.
        for reason in [MissingReason::Unavailable, MissingReason::Denied] {
            let text = bytes_cell(&Observed::Missing(reason));
            assert!(!text.chars().any(|character| character.is_ascii_digit()));
            for unit in IEC_UNITS {
                assert!(!text.ends_with(unit), "{text}");
            }
        }
    }

    #[test]
    fn no_published_cell_is_wider_than_the_reserved_bound() {
        assert_eq!(
            missing_text(MissingReason::Unavailable).len(),
            MAX_CELL_BYTES
        );
        let mut widest = 0;
        // Every multiple boundary, the value just below it, and every power of
        // two in between: the widest value is 1023.9 of some unit, which this
        // walk reaches for each of them.
        for exponent in 0..64u32 {
            for bytes in [
                1u64 << exponent,
                (1u64 << exponent).wrapping_sub(1),
                (1u64 << exponent).wrapping_add(1),
            ] {
                let cell = format_iec_bytes(bytes);
                widest = widest.max(cell.len());
                assert!(cell.len() <= MAX_CELL_BYTES, "{bytes} formatted as {cell}");
            }
        }
        assert_eq!(
            format_iec_bytes(u64::MAX).len().max(widest),
            10,
            "the widest number this formatter emits is `1023.9 PiB`"
        );
    }

    fn interval(ticks: u64, ticks_per_second: u64, elapsed: std::time::Duration) -> CpuUsage {
        CpuUsage::Measured(CpuInterval {
            ticks,
            ticks_per_second,
            elapsed,
        })
    }

    #[test]
    fn one_fully_used_logical_cpu_is_one_hundred_percent_and_more_cpus_exceed_it() {
        use std::time::Duration;
        for (ticks, hz, elapsed, expected) in [
            // Half of one CPU at USER_HZ 100 over one second.
            (50, 100, Duration::from_secs(1), "50.0%"),
            // One CPU, fully used, over two seconds.
            (200, 100, Duration::from_secs(2), "100.0%"),
            // Four CPUs fully used by one multithreaded process.
            (400, 100, Duration::from_secs(1), "400.0%"),
            // A different tick rate is a different divisor, not a different scale.
            (1024, 1024, Duration::from_secs(1), "100.0%"),
            // A known zero is a measured zero.
            (0, 100, Duration::from_secs(1), "0.0%"),
            // Truncated, never rounded up: 1/3 of a CPU is 33.3%, and one tick
            // short of a full CPU is not displayed as one.
            (1, 100, Duration::from_millis(30), "33.3%"),
            (1999, 100, Duration::from_secs(20), "99.9%"),
            (999, 100, Duration::from_secs(10), "99.9%"),
        ] {
            assert_eq!(
                cpu_cell(&interval(ticks, hz, elapsed)),
                expected,
                "{ticks} ticks at {hz} Hz over {elapsed:?}"
            );
        }
    }

    /// PX-006 review round 1 (W3): at the shortest refresh interval one clock
    /// tick is a large share of the interval. At 100 Hz over 50 ms a tick is 20
    /// percentage points, so a fully busy thread honestly reads 80%, 100% or
    /// 120% depending on where tick boundaries fell. This pins the arithmetic;
    /// the quantization is documented, not smoothed.
    #[test]
    fn a_short_interval_is_quantized_by_the_tick_rate() {
        use std::time::Duration;
        let fifty = Duration::from_millis(50);
        for (ticks, expected) in [(0, "0.0%"), (4, "80.0%"), (5, "100.0%"), (6, "120.0%")] {
            assert_eq!(cpu_cell(&interval(ticks, 100, fifty)), expected);
        }
    }

    #[test]
    fn an_interval_that_cannot_be_divided_by_is_unavailable_rather_than_a_spike() {
        use std::time::Duration;
        // Zero elapsed time would divide by zero; a zero tick rate likewise.
        assert_eq!(cpu_cell(&interval(10, 100, Duration::ZERO)), "Unavailable");
        assert_eq!(
            cpu_cell(&interval(10, 0, Duration::from_secs(1))),
            "Unavailable"
        );
        // A delta implying more logical CPUs than any host has is a counter
        // discontinuity, not usage: refused rather than published as a spike.
        let ceiling = MAX_PLAUSIBLE_LOGICAL_CPUS * 100;
        assert_eq!(
            cpu_cell(&interval(ceiling, 100, Duration::from_secs(1))),
            "6553600.0%"
        );
        assert_eq!(
            cpu_cell(&interval(ceiling + 1, 100, Duration::from_secs(1))),
            "Unavailable"
        );
        assert_eq!(
            cpu_cell(&interval(u64::MAX, u64::MAX, Duration::MAX)),
            "Unavailable",
            "an overflowing divisor is refused, never wrapped"
        );
        assert_eq!(
            cpu_cell(&interval(u64::MAX, 1, Duration::from_nanos(1))),
            "Unavailable"
        );
    }

    #[test]
    fn warming_up_and_unread_cpu_are_never_published_as_zero() {
        assert_eq!(cpu_cell(&CpuUsage::WarmingUp), "Warming up");
        assert_eq!(
            cpu_cell(&CpuUsage::Missing(MissingReason::Denied)),
            "Denied"
        );
        assert_eq!(
            cpu_cell(&CpuUsage::Missing(MissingReason::Unavailable)),
            "Unavailable"
        );
        for state in [
            CpuUsage::WarmingUp,
            CpuUsage::Missing(MissingReason::Denied),
            CpuUsage::Missing(MissingReason::Unavailable),
        ] {
            let text = cpu_cell(&state);
            assert!(!text.chars().any(|character| character.is_ascii_digit()));
            assert!(!text.ends_with('%'), "{text}");
        }
    }

    #[test]
    fn no_cpu_cell_is_wider_than_the_reserved_bound() {
        use std::time::Duration;
        assert!(CPU_WARMING_UP.len() <= MAX_CELL_BYTES);
        let widest = cpu_cell(&interval(
            MAX_PLAUSIBLE_LOGICAL_CPUS * 100,
            100,
            Duration::from_secs(1),
        ));
        assert_eq!(widest.len(), 10);
        assert!(widest.len() <= MAX_CELL_BYTES);
        for tenths in [0, 9, 10, 999, 1_000, 65_535_999, 65_536_000] {
            assert!(format_cpu_tenths(tenths).len() <= MAX_CELL_BYTES);
        }
    }

    #[test]
    fn the_cpu_definition_names_its_convention_and_its_interfaces() {
        let metric = CPU_USAGE;
        assert!(metric.label.contains("100% = 1 CPU"), "{}", metric.label);
        assert!(metric.source.contains("fields 14 (utime) and 15 (stime)"));
        assert!(metric.source.contains("AT_CLKTCK"));
        assert!(metric.source.contains("monotonic"));
        assert!(metric.interpretation.contains("may exceed 100%"));
        assert!(metric.interpretation.contains("never as zero"));
    }

    #[test]
    fn shares_are_truncated_tenths_and_an_undefined_share_is_none() {
        assert_eq!(share_tenths(1, 3), Some(333));
        assert_eq!(share_tenths(2, 3), Some(666), "66.66…% truncates to 66.6%");
        assert_eq!(share_tenths(0, 7), Some(0), "a measured zero is a value");
        assert_eq!(share_tenths(7, 7), Some(1_000));
        assert_eq!(share_tenths(u64::MAX, u64::MAX), Some(1_000));
        assert_eq!(share_tenths(u64::MAX - 1, u64::MAX), Some(999));
        // Nothing to divide by, and a part larger than its whole, are not shares.
        assert_eq!(share_tenths(0, 0), None);
        assert_eq!(share_tenths(8, 7), None);
    }

    #[test]
    fn utc_times_are_formatted_exactly_and_never_before_the_epoch() {
        use std::time::Duration;
        let at = |seconds: u64| UNIX_EPOCH + Duration::from_secs(seconds);
        for (seconds, expected) in [
            (0, "1970-01-01 00:00:00 UTC"),
            (1_800_000_000, "2027-01-15 08:00:00 UTC"),
            // Leap days, a century leap year and the turn of a year.
            (951_782_400, "2000-02-29 00:00:00 UTC"),
            (1_709_210_096, "2024-02-29 12:34:56 UTC"),
            (1_735_689_599, "2024-12-31 23:59:59 UTC"),
            (1_735_689_600, "2025-01-01 00:00:00 UTC"),
            (4_107_542_400, "2100-03-01 00:00:00 UTC"),
            (253_402_300_799, "9999-12-31 23:59:59 UTC"),
            (253_402_300_800, "10000-01-01 00:00:00 UTC"),
        ] {
            assert_eq!(
                format_utc(at(seconds)).as_deref(),
                Some(expected),
                "{seconds}"
            );
        }
        // Truncated to the second, never rounded up into the next one.
        assert_eq!(
            format_utc(at(59) + Duration::from_nanos(999_999_999)).as_deref(),
            Some("1970-01-01 00:00:59 UTC")
        );
        assert_eq!(format_utc(UNIX_EPOCH - Duration::from_secs(1)), None);
    }

    #[test]
    fn uptime_is_truncated_to_whole_minutes() {
        for (seconds, expected) in [
            (0, "0 h 00 min"),
            (59, "0 h 00 min"),
            (60, "0 h 01 min"),
            (86_399, "23 h 59 min"),
            (86_400, "1 day, 0 h 00 min"),
            (2 * 86_400 + 3_599, "2 days, 0 h 59 min"),
            (273_906, "3 days, 4 h 05 min"),
        ] {
            assert_eq!(format_uptime(seconds), expected, "{seconds}");
        }
        assert!(format_uptime(u64::MAX).starts_with("213503982334601 days, "));
    }

    #[test]
    fn load_averages_keep_the_kernels_two_decimals() {
        for (hundredths, expected) in [
            (0, "0.00"),
            (5, "0.05"),
            (52, "0.52"),
            (125, "1.25"),
            (1_200, "12.00"),
        ] {
            assert_eq!(format_hundredths(hundredths), expected);
        }
    }

    /// PX-007: each system figure states which file and fields it is read
    /// from, how it is computed, and — for a percentage — its denominator.
    #[test]
    fn the_system_definitions_name_their_files_fields_and_denominators() {
        assert_eq!(SYSTEM_CPU.label, "Overall CPU (100% = all logical CPUs)");
        assert_ne!(SYSTEM_CPU.label, CPU_USAGE.label);
        for field in [
            "/proc/stat",
            "user",
            "nice",
            "system",
            "idle",
            "iowait",
            "irq",
            "softirq",
            "steal",
            "cpuN",
        ] {
            assert!(SYSTEM_CPU.source.contains(field), "{field}");
        }
        assert!(SYSTEM_CPU
            .interpretation
            .contains("Busy is user + nice + system + irq + softirq + steal"));
        assert!(SYSTEM_CPU.interpretation.contains("idle is idle + iowait"));
        assert!(SYSTEM_CPU.interpretation.contains("guest and guest_nice"));
        assert!(SYSTEM_CPU.interpretation.contains("never as zero"));
        assert!(MEMORY_USED.source.contains("MemTotal and MemAvailable"));
        assert!(MEMORY_USED
            .interpretation
            .contains("Used is MemTotal - MemAvailable"));
        assert!(MEMORY_USED.interpretation.contains("used / MemTotal"));
        assert!(SWAP_USED.source.contains("SwapTotal and SwapFree"));
        assert!(SWAP_USED.interpretation.contains("never 0% of 0"));
        assert!(UPTIME.source.contains("/proc/uptime"));
        assert!(LOAD_AVERAGE.source.contains("/proc/loadavg"));
        assert!(LOAD_AVERAGE.interpretation.contains("Not a percentage"));
        assert!(PROCESS_COUNT.label.contains("visible to this reader"));
        assert!(PROCESS_COUNT.interpretation.contains("PID namespace"));
        assert!(PROCESS_COUNT.interpretation.contains("hidepid"));
        assert!(PROCESS_COUNT
            .interpretation
            .contains("applies no filter of its own"));
        assert!(SYSTEM_CPU
            .interpretation
            .contains("two consecutive successful reads"));
        assert!(SAMPLE_TIME.interpretation.contains("UTC"));
        assert!(SAMPLE_TIME.interpretation.contains("transport loss"));
    }

    #[test]
    fn the_definition_states_the_interface_it_reads_and_what_it_omits() {
        let metric = RESIDENT_MEMORY;
        assert_eq!(metric.unit, "bytes");
        assert!(metric
            .source
            .contains("/proc/<pid>/statm field 2 (resident)"));
        assert!(metric.source.contains("AT_PAGESZ"));
        assert!(metric.interpretation.contains("Shared pages"));
        assert!(metric.interpretation.contains("swapped out"));
        // Where it is exact, where it is not, and how far off it can be.
        assert!(metric.interpretation.contains("Exact from Linux 6.16"));
        assert!(metric.interpretation.contains("max(32, 2 x online CPUs)"));
        assert!(metric.interpretation.contains("can read 0 B"));
        assert!(metric.interpretation.contains("field 24"));
        assert!(metric.interpretation.contains("Before 6.2"));
        // And that the same-directory read depends on the reader's own /proc.
        assert!(metric.source.contains("by name where it"));
    }
}
