# R1 VoiceOver and keyboard checklist (PX-026-G01)

About ten minutes, on the R1 build, by a person. The automated part of this check is
`ProcessExplorerR1DevboxTests.theLivePackageOffersNoProcessChangingActionAndIsLabelledAndKeyboardOperable`
(every control labelled with a role, every shortcut bound, the keyboard workflow against live data)
and PX-016's `theBasicWorkflowRunsFromTheKeyboardWithDiscoverableAccessibleShortcuts`. What no test
can hear is what VoiceOver actually says, and what a person sees on screen. That is this list.

The gate does not close until this file comes back filled in. Do not mark an item PASS unless you
saw or heard it yourself.

## Set up

1. Any macOS account with Docker Desktop running; the clean account from the walkthrough is fine.
   Stop the devbox in any other account first, because it uses port 2222.
2. From the checkout, run:

   ```sh
   apps/srtop/r1/checklist-session.sh
   ```

   It starts the devbox and installs the client if it is not installed. It asks before it adds a
   marked `srtop-devbox` block to `~/.ssh/config`, and it removes exactly that block when you press
   Enter at the end. It then opens the client's Connections window.
3. Turn VoiceOver on with Command-F5. Turn it off the same way when you are done.
4. Keep a second Terminal window open in the checkout for the two `devbox.sh` commands below.

Record each line as `PASS`, `FAIL` or `N/A`. Add a short note for anything that is not PASS.

## Items

| # | Do this | Expect | Result | Note |
|---|---|---|---|---|
| 1 | In the Connections window, open the Connect form (Add Connection). Type `host;id` in Host and `srui` in Remote User. | Connect is disabled. A red reason names the `;` character. VoiceOver reads the reason when you move to it. Nothing connects. | | |
| 2 | Clear Host and type `srtop-devbox`. Leave Remote User as `srui`. Press Return. Press Open on the entry if the window has not opened by itself. | Connect is the default button. The entry reads "connected". The Process Explorer window shows live rows (sshd, srtop and others). No host-key prompt or "trust" offer appears. | | |
| 3 | With the Process Explorer window focused, press Command-L, then Down Arrow a few times. | VoiceOver announces the table and each row as you move. The inspector below names the selected PID. | | |
| 4 | Press Command-F and type `ssh`. Then press Command-G. | Focus moves to the search field, and VoiceOver reads its label. The typed letters are text, not shortcuts. Command-G moves to the next match. | | |
| 5 | Press Tab and Shift-Tab through the window. | Each control you land on is announced with a label and a role, for example "Pause view, button". No control is silent or reads only "button". | | |
| 6 | Move the VoiceOver cursor to a few buttons, such as Pause view, Refresh now and Sort by CPU. | VoiceOver reads each one's keyboard shortcut in its help text, for example "Keyboard shortcut ⌥⌘P". | | |
| 7 | Click a row, so the inspector shows it. In the second Terminal, run `apps/srtop/devbox/devbox.sh drop-bridges`. | Within a few seconds the window's subtitle reads "disconnected — showing data from revision N (stale)", and VoiceOver announces it. The rows stay on screen. | | |
| 8 | While it is disconnected, click a different row. | The highlight does not move to the clicked row. A stale window takes no new selection. | | |
| 9 | Reconnect it: press Connect on the entry in the Connections window, which reads "disconnected — resume will be attempted". | The subtitle goes through "connecting — …" and then clears. VoiceOver announces the change. The rows update again, and the earlier selection is still on its process. | | |
| 10 | In the second Terminal, run `apps/srtop/devbox/devbox.sh restart-srtop`, then reconnect as in item 9. | A blocking "Session Replaced" alert names the old and new session IDs, and VoiceOver reads it. The subtitle reads "session replaced — rebuilding", then clears. No old selection survives. | | |
| 11 | Look through every button and menu in the window. | Nothing offers to kill, signal, stop, renice or otherwise change a process. R1 is read-only. | | |
| 12 | Quit the client with Command-Q. In the checklist Terminal, press Enter. | The client quits. The script reports that it removed the alias and stopped the box. | | |

## Send back

- This file, filled in.
- macOS version (`sw_vers -productVersion`) and the checkout's revision (`git rev-parse HEAD`).
- Optional: a screenshot of the window after item 7, and one after item 10. Capture the client
  window only, never the whole desktop.

Tester: ______  Date: ______  macOS: ______  Revision: ______
