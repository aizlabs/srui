//! Signal-target conversion coverage (§27).
//!
//! These tests exercise the validation boundary only; the real signal API is never called.

use srui_example_process_monitor::{signal_target_pid, TerminateError};

#[test]
fn ordinary_pids_convert_to_themselves() {
    for pid in [1u32, 2, 4_242, 99_999, i32::MAX as u32] {
        assert_eq!(signal_target_pid(pid), Ok(pid as i32));
    }
}

#[test]
fn pid_zero_is_rejected_because_it_means_the_callers_process_group() {
    assert_eq!(
        signal_target_pid(0),
        Err(TerminateError::InvalidTarget(0)),
        "kill(2) reads 0 as 'every process in my process group'"
    );
}

#[test]
fn values_above_i32_max_are_rejected_instead_of_wrapping_negative() {
    for pid in [i32::MAX as u32 + 1, u32::MAX - 1, u32::MAX] {
        assert_eq!(
            signal_target_pid(pid),
            Err(TerminateError::InvalidTarget(pid)),
            "a wrapped {pid} would acquire process-group or broadcast semantics"
        );
        assert!(
            (pid as i32) <= 0,
            "sanity: {pid} really does wrap non-positive"
        );
    }
}

#[test]
fn an_invalid_target_reports_a_readable_refusal() {
    assert_eq!(
        TerminateError::InvalidTarget(0).to_string(),
        "0 is not a single-process signal target"
    );
}
