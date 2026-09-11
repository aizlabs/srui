"""Shared benchmark-runner exceptions."""


class BenchmarkError(RuntimeError):
    """The benchmark contract or execution failed."""


class CandidateCleanupAssertionError(BenchmarkError):
    """The renderer-candidate cleanup probe reached a failing outcome.

    Separated from its enclosing `BenchmarkError` so the run can record
    `candidate_failure_cleanup: passed=false` in a real report instead of
    aborting before one is written. Infrastructure faults inside the same
    probe -- a missing release driver, a supervision failure, a timeout --
    stay plain `BenchmarkError` and still abort, because they prove nothing
    either way about whether a candidate leaked a child.
    """
