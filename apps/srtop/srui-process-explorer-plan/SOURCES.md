## References and provenance

Checked 11 September 2026. The htop tag is the feature baseline, not a request to copy its source. Task designs, staging, budgets, and acceptance criteria are proposed engineering work, not upstream claims. Version-specific OS/API details must be verified again when each adapter is implemented. The original two documents are not reproduced in this pack.

- **[D1] User-supplied SRUI Semantic Remote UI design, v0.6, dated 29 August 2026.** `SRUI_Semantic_Remote_UI_Design_v0.6.md` at the repository root.
- **[D2] User-supplied sequential implementation plan T0–T38.** `SRUI_Implementation_Plan.md` at the repository root; completion is established by the revision-specific baseline, not by ticket numbering.
- **[H1] htop official project and releases; 3.5.3 was marked Latest when checked.** `https://htop.dev/ ; https://github.com/htop-dev/htop/releases`
- **[H2] htop 3.5.3 manual: user workflows, options and PCP variant.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/htop.1.in`
- **[H3] htop 3.5.3 Linux process-field registry.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/linux/LinuxProcess.c`
- **[H4] htop 3.5.3 Linux platform: meters and platform actions.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/linux/Platform.c`
- **[H5] htop 3.5.3 common action registry.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/Action.c`
- **[H6] htop 3.5.3 display-option definitions.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/DisplayOptionsPanel.c`
- **[H7] htop 3.5.3 scheduling implementation.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/Scheduling.c`
- **[H8] htop 3.5.3 changelog; corroborates optional and recent feature coverage.** `https://raw.githubusercontent.com/htop-dev/htop/3.5.3/ChangeLog`
- **[K1] Linux kernel /proc documentation: process data and field semantics.** `https://docs.kernel.org/filesystems/proc.html`
- **[K2] Linux man-pages: pidfd_send_signal.** `https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html`
- **[K3] Linux man-pages: pidfd_open.** `https://man7.org/linux/man-pages/man2/pidfd_open.2.html`
- **[K4] Linux man-pages: sched_setaffinity.** `https://man7.org/linux/man-pages/man2/sched_setaffinity.2.html`
- **[K5] Linux man-pages: getpriority/setpriority.** `https://man7.org/linux/man-pages/man2/setpriority.2.html`
- **[K6] Linux kernel: Pressure Stall Information.** `https://docs.kernel.org/accounting/psi.html`
- **[S1] sysinfo Process API; implementation must use the repository-pinned version, not assume latest API compatibility.** `https://docs.rs/sysinfo/latest/sysinfo/struct.Process.html`

- **[BT1] btop++ by aristocratos — dashboard inspiration.** [Upstream repository](https://github.com/aristocratos/btop). Pin a reference tag/commit and selected workflows at PX-070-G01. No latest-version, feature-parity, or performance claim was verified for this amendment.
