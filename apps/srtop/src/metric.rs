//! Metric definitions: what one published number is, where it was read, and how
//! it must be read back (design §§6.2, 22, 29; PX-005 resident memory).
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
use crate::source::{MissingReason, Observed};

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
    source: "Linux /proc/<pid>/stat field 24 (rss), in pages, multiplied by the page size the \
            kernel reports through AT_PAGESZ in /proc/self/auxv of the scanned mount (K1)",
    interpretation: "Pages this process has in real memory at sample time. Shared pages are \
                     counted in full for every process that maps them, so these values do not sum \
                     to the memory a host has in use; pages that are swapped out or were never \
                     faulted in are not counted at all. The kernel publishes the same number as \
                     `resident` in /proc/<pid>/statm and as `VmRSS` in /proc/<pid>/status, and \
                     documents all three as inaccurate. Proportional and shared-page accounting \
                     is PX-046, not this metric.",
};

/// IEC binary multiples, in ascending order. Each is 1024 times the one before,
/// so converting between them is a shift (§22: a published unit is never
/// ambiguous — `MiB` is 1024², never 10⁶).
const IEC_UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// The longest text [`bytes_cell`] can publish, in encoded bytes.
///
/// It is [`missing_text`]'s longest wording, not a number: the widest value this
/// formatter can emit is `"1023.9 PiB"` at ten bytes. The bound is asserted
/// against both by `no_published_cell_is_wider_than_the_reserved_bound`, and
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

    #[test]
    fn the_definition_states_the_interface_it_reads_and_what_it_omits() {
        let metric = RESIDENT_MEMORY;
        assert_eq!(metric.unit, "bytes");
        assert!(metric.source.contains("/proc/<pid>/stat field 24"));
        assert!(metric.source.contains("AT_PAGESZ"));
        assert!(metric.interpretation.contains("Shared pages"));
        assert!(metric.interpretation.contains("swapped out"));
    }
}
