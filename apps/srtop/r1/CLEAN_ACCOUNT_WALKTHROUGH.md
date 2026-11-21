# R1 clean-account walkthrough: how to run it (PX-026-G01)

The R1 gate needs one run of `clean-account-walkthrough.sh` in a macOS account that has never built
SRUI. Only you can create that account, so the gate waits on the result file this produces.

The script never uses `sudo`. It never touches `~/.ssh` or a system setting, and writes only in that
account's home and in the checkout it runs from. The script's header lists every place it writes.
It takes about 30 to 45 minutes, most of it the two release builds, the devbox image and the native
test build.

## 1. Prepare, in your usual account

1. Quit Docker Desktop in your usual account, so the new account's Docker can start and use port
   2222. If the devbox is running, stop it first with `apps/srtop/devbox/devbox.sh down`.
2. Make the repository available to the new account as a git bundle. Run this in the checkout:

   ```sh
   git bundle create /Users/Shared/srui-r1.bundle codex/PX-026-G01-r1-package
   ```

   If the branch has been merged or pushed by then, clone it from GitHub in step 3 instead.

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

## 3. Run it, in the new account

Open Terminal and run:

```sh
git clone --branch codex/PX-026-G01-r1-package /Users/Shared/srui-r1.bundle ~/srui-r1-src
cd ~/srui-r1-src
apps/srtop/r1/clean-account-walkthrough.sh
```

During the run:

- A Process Explorer window opens briefly, twice, as the installed client connects to the devbox.
- The native tests open and close windows of their own.
- Leave the account logged in and the display awake until it finishes.

It prints every step as it goes, and ends with the verdict and the result file's name:

```
SUMMARY PASS mode=real steps=17 pass=17 fail=0 skip=0
CHECKSUM sha256=...
The walkthrough passed. Send back: ~/srui-r1-walkthrough-<UTC time>.txt
```

If a step fails, it keeps going to the end, so the uninstall and the data check still run. The
step logs stay in `~/srui-r1-walkthrough/logs` for you to look at. Running it again starts from the
beginning.

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

- the clone, `~/srui-r1-src`;
- the base images Docker pulled to build the devbox (`rust:1-bookworm` and `debian:bookworm-slim`,
  pinned by digest);
- the whole test account. Delete it in System Settings, Users & Groups.

The VoiceOver and keyboard checklist is a separate, manual check:
[voiceover-keyboard-checklist.md](voiceover-keyboard-checklist.md). It can run in this account
before you delete it.
