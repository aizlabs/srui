use std::process::Command;

#[test]
fn help_flags_exit_successfully_and_print_usage_to_stdout() {
    for flag in ["-h", "--help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ui-gallery"))
            .arg(flag)
            .output()
            .expect("ui-gallery help command runs");
        assert!(
            output.status.success(),
            "{flag} must exit successfully, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("usage: ui-gallery"),
            "{flag} must print usage to stdout"
        );
        assert!(
            output.stderr.is_empty(),
            "{flag} must not be reported as an error"
        );
    }
}
