# R1 clean-account walkthrough: how to run it (PX-026-G01, PX-027)

The R1 gates need one run of `clean-account-walkthrough.sh --with-showcase` in a macOS account that
has never built SRUI. Only you can create that account, so both gates wait on the one result file
this produces. PX-026-G01 needs the walkthrough itself. PX-027, the R1 showcase gate, needs the
one extra step that `--with-showcase` adds (section 3).

The script never uses `sudo`. It never touches `~/.ssh` or a system setting, and writes only in that
account's home and in the checkout it runs from. The script's header lists every place it writes.
It takes about 40 to 55 minutes, most of it the two release builds, the devbox image, the native
test build and the showcase step.

**Run it from `main` once PX-027 has merged.** `main` then has both gates' code, PX-026-G02's
server refusal (without it the `r1-live-checks` step fails) and `--with-showcase` (without it the
script stops with `unknown option: --with-showcase`, exit 2). Before PX-027 merges, use the branch
`codex/PX-027-showcase` instead of `main` in the commands below.

The script runs only in a fresh account. It refuses, before writing anything, an account that
already has any of these:

- an R1 install (`~/.local/share/srui-r1`);
- saved connections (`~/Library/Application Support/SRUI/saved-connections.json` or its
  `.unreadable` backup);
- devbox state (`~/.codex/srtop-devbox`);
- a `srtop-devbox` Docker container.

That is why it must not run in your usual account. `--allow-existing-srui-data` overrides the
refusal and is dangerous: the run then replaces and removes the existing install and container.
It is meant for throwaway accounts only, and this walkthrough does not need it.

## 1. Prepare, in your usual account

1. Quit Docker Desktop in your usual account, so the new account's Docker can start and use port
   2222. If the devbox is running, stop it first with `apps/srtop/devbox/devbox.sh down`.
2. Only if the new account cannot clone from GitHub, make a git bundle instead. A bundle must hold a
   local branch, because a bundle of `origin/main` holds only `refs/remotes/origin/main`, which a
   clone cannot check out. Run this in your checkout:

   ```sh
   git fetch origin
   git branch -f srui-r1 origin/main
   git bundle create /Users/Shared/srui-r1.bundle srui-r1
   ```

   Before PX-027 merges, use `git branch -f srui-r1 origin/codex/PX-027-showcase` instead. The
   bundle always holds the branch `srui-r1`.

## 2. Create the account

1. Open System Settings, go to Users & Groups, and choose Add User, for example "SRUI R1 Test".
   Make it an Administrator account. Docker Desktop's first start installs its own components
   with administrator approval. The privacy check reads the client's unified log with `log show`,
   which may refuse a standard account; this was not tested in one. The script itself never asks
   for or uses administrator rights.
2. Log out, or use fast user switching, and log in to the new account. Finish the setup
   assistant; you can skip Apple Account sign-in.
3. In the new account, open Docker Desktop from /Applications once and accept its terms. If it asks
   an administrator to install its own components, that is Docker's installer, not this script's.
   Wait until it says the engine is running.
4. Xcode is shared by every account. If `swift --version` in Terminal asks you to accept a license,
   accept it.
5. Install Rust for this account only. The showcase step builds its measuring tool with `cargo`.
   rustup installs into `~/.rustup` and `~/.cargo`, needs no `sudo` and changes no shell profile
   with `--no-modify-path`:

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
   . "$HOME/.cargo/env"
   cargo --version
   ```

   Run `. "$HOME/.cargo/env"` again in any new Terminal window before step 3. The build downloads
   its crates from crates.io, so the account needs network access during the run, as it does for
   the Docker base images.

## 3. Run it, in the new account

Open Terminal and run:

```sh
. "$HOME/.cargo/env"
git clone --branch main https://github.com/aizlabs/srui.git ~/srui-r1-src
cd ~/srui-r1-src
apps/srtop/r1/clean-account-walkthrough.sh --with-showcase
```

With the bundle instead of GitHub, clone the bundle's `srui-r1` branch:

```sh
git clone --branch srui-r1 /Users/Shared/srui-r1.bundle ~/srui-r1-src
```

During the run:

- A Process Explorer window opens briefly, twice, as the installed client connects to the devbox.
- The native tests open and close windows of their own.
- After the live checks, the `px027-showcase` step runs `apps/srtop/r1/showcase-measure.sh --quick`
  against the same box. An unchanged fake source must send no app transaction, the demonstration
  workers must be gone after every run, and srtop is measured against htop and at four RTTs. Then
  the box goes back on srtop's default source for the later steps.
- Leave the account logged in and the display awake until it finishes.

It prints every step as it goes, and ends with the verdict and the result file's name:

```
SUMMARY PASS mode=real steps=18 pass=18 fail=0 skip=0
CHECKSUM sha256=...
The walkthrough passed. Send back: ~/srui-r1-walkthrough-<UTC time>.txt
```

Without `--with-showcase` the run has 17 steps and serves PX-026-G01 only. Without `cargo` on the
`PATH`, `--with-showcase` fails the prerequisites step and the run stops there, before anything is
planted, built or installed.

If the preflight or the prerequisites step fails, the run stops there, before anything is
planted, built or installed. It ends with `STOPPED after …` and `SUMMARY FAIL`; fix what that step
names and run it again.

If a later step fails, the run keeps going to the end, so the uninstall and the data check still
run. The step logs stay in `~/srui-r1-walkthrough/logs` for you to look at. Running it again
starts from the beginning.

## 4. Send back

- The one result file, `~/srui-r1-walkthrough-<UTC time>.txt`. To hand it over, copy it to
  `/Users/Shared/`.

The file holds timestamps, the commands run, exit codes and versions. Your home directory appears
as `~`, the checkout as `<checkout>` and the host name as `<host>`. It holds no key, token or
environment dump; the script audits the file for those before finishing. Read it before you send
it. You can check that it is complete and unedited yourself:

```sh
apps/srtop/r1/clean-account-walkthrough.sh --verify ~/srui-r1-walkthrough-<UTC time>.txt
```

## 5. Afterwards

The walkthrough has already uninstalled what it installed. It also removed the devbox container and
image, and the devbox state it created. Some things remain, and you can remove them:

- the clone, `~/srui-r1-src`, which also holds the showcase tool's build in
  `apps/srtop/demo/target`;
- Rust: `~/.rustup` and `~/.cargo` (the toolchain and the downloaded crates). `rustup self
  uninstall` removes both;
- the base images Docker pulled to build the devbox (`rust:1-bookworm` and `debian:bookworm-slim`,
  pinned by digest);
- the whole test account. Delete it in System Settings, Users & Groups.

The VoiceOver and keyboard checklist is a separate, manual check:
[voiceover-keyboard-checklist.md](voiceover-keyboard-checklist.md). It can run in this account
before you delete it.
