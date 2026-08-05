# Build automation: closing the gap between README prose and real scripts

Every cross-compile path in this repo works today, but "works" currently means "a human follows README.md's
prose instructions, in order, on a fresh machine, and doesn't mistype a URL." This document lays out what's
needed to turn that into real, idempotent setup scripts — what exists already, what's still manual, and a
concrete design for closing the gap. Documentation only; no scripts have been written yet.

## 1. What's already automated

Worth being precise about this, since it's not starting from zero:

- **`cargo build`** itself needs no manual steps at all for the crates that apply to the host you're already
  on — `browser-core`, `render-engine`, and whichever frontend crate matches your `target_os` compile
  natively with nothing beyond a working Rust install.
- **`.cargo/build-macos-appkit.sh`** already automates *discovery* of an already-set-up Zig/SDK (searches
  `.zig/`/`.macos-sdk/`, falls back to `PATH`/`SDKROOT`) — it just doesn't automate the initial *fetching* of
  those two things. This is the closest existing precedent for the scripts proposed below; §3 follows its
  shape deliberately.
- **`cargo build-windows-reactor`** (a `.cargo/config.toml` alias) fully automates the xwin *invocation* —
  `cargo-xwin` itself downloads and caches the Windows SDK + MSVC CRT on first use, no manual step there.
  What's still manual is getting `cargo-xwin` and a system `clang`/`lld`/`llvm-lib` installed in the first
  place (§2.2).
- **`.github/workflows/windows.yml`/`macos.yml`** are themselves fully automated environment setup + build,
  just scoped to GitHub's hosted runners specifically (`windows-latest` already has the real MSVC toolchain;
  `macos-14`/`macos-13` already have real Xcode) — they don't help a fresh Linux dev machine.

## 2. What's still manual, precisely

Enumerated from README.md's current instructions — this is the actual scope of what needs scripting.

### 2.1 Linux native (`browser-core`, `browser-linux-gtk3`)

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # if no Rust at all
sudo apt install -y build-essential libgtk-3-dev libwebkit2gtk-4.1-dev  # or -4.0-dev
```

Smallest gap of the four — two package-manager calls. Still worth scripting for consistency and to handle
the `-4.1-dev`/`-4.0-dev` fallback automatically rather than making a human retry by hand.

### 2.2 Windows cross-compile (`browser-windows-reactor`)

```sh
cargo install cargo-xwin
rustup target add x86_64-pc-windows-msvc
sudo apt install -y clang-21 lld-21 llvm-21   # exact package names checked against this machine
```

Plus the part that's *not* in README.md at all today because it's genuinely machine-specific and
deliberately kept out of the repo-tracked `.cargo/config.toml` (see that file's own comment on why): setting
`CC_x86_64_pc_windows_msvc`/`AR_x86_64_pc_windows_msvc` in `~/.cargo/config.toml` to wherever this
distro/version actually put `clang-cl`/`llvm-lib` (`/usr/lib/llvm-21/bin/...` here, but the versioned
package name — `llvm-21` today — drifts over time and across distros). This is the one step in the whole
document that's an actual *gap* right now, not just unscripted: there's no automated way to discover this
path today; a human has to `dpkg -l | grep llvm`/`which clang-cl` by hand and edit `~/.cargo/config.toml`
themselves.

### 2.3 macOS cross-compile (`browser-macos-appkit`)

```sh
cargo install cargo-zigbuild
rustup target add aarch64-apple-darwin x86_64-apple-darwin
curl -fsSL -o /tmp/zig.tar.xz https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz
mkdir -p .zig && tar -C .zig -xf /tmp/zig.tar.xz
mkdir -p .macos-sdk
curl -fsSL -o /tmp/macos-sdk.tar.xz https://github.com/joseluisq/macosx-sdks/releases/download/14.0/MacOSX14.0.sdk.tar.xz
tar -C .macos-sdk -xf /tmp/macos-sdk.tar.xz
```

The largest gap — five real setup steps, a ~1.5 GB SDK download, and (see §4) a licensing caveat that means
this one specifically should *ask*, not silently fetch, on a machine that's never done this before.

## 3. Proposed structure

A `scripts/` directory at the repo root, one setup script per cross-compile target plus an orchestrator —
matching `.cargo/build-macos-appkit.sh`'s existing conventions (`set -eu`, resolve paths from the script's
own location so it works from any invocation directory, check-before-fetching so re-running is always safe):

```
scripts/
  setup-linux-native.sh     # apt packages for browser-core/browser-linux-gtk3
  setup-windows-cross.sh    # cargo-xwin, target, clang/lld/llvm-21, CC/AR discovery
  setup-macos-cross.sh      # cargo-zigbuild, targets, Zig, macOS SDK (interactive on first run — see §4)
  build-all.sh              # runs whichever of the above apply, then builds every target
```

### 3.1 `setup-linux-native.sh`

Thinnest of the four — detect whether `libwebkit2gtk-4.1-dev` is installable, fall back to `-4.0-dev`
automatically (something the current README just tells a human to notice and retry), otherwise a direct
translation of §2.1. No real design decisions here.

### 3.2 `setup-windows-cross.sh`

The one genuine *new* capability this needs beyond translating README prose: automated discovery of
`clang-cl`/`llvm-lib`'s real path after installing `clang-21`/`lld-21`/`llvm-21` (package name may drift —
script should discover the installed version via `dpkg -l` rather than hardcoding `-21`), then write
`CC_x86_64_pc_windows_msvc`/`AR_x86_64_pc_windows_msvc` into `~/.cargo/config.toml` — appending to the file
if it exists, not overwriting it, and checking first whether those exact keys are already set correctly
(idempotent re-runs). This directly closes the one real gap identified in §2.2.

### 3.3 `setup-macos-cross.sh`

Same shape as `.cargo/build-macos-appkit.sh`'s existing discovery logic, but for the *fetch* side: install
`cargo-zigbuild`, add the two Rust targets, download+extract Zig into `.zig/` if not already there, and
download+extract the macOS SDK into `.macos-sdk/` if not already there — **except the SDK step needs an
explicit, interactive confirmation the first time**, not a silent fetch. See §4 for why this is a real
design requirement, not caution for its own sake.

### 3.4 `build-all.sh`

Detects which setup scripts are relevant (native Linux frontend always; Windows/macOS cross-compiles if
their setup has already been run, or runs it first if `--setup` is passed), then runs the full matrix:

```sh
cargo build                                       # native (whatever this host is)
cargo build-windows-reactor                       # if Windows toolchain present
.cargo/build-macos-appkit.sh aarch64-apple-darwin # if macOS toolchain present
.cargo/build-macos-appkit.sh x86_64-apple-darwin
```

Non-zero exit if any step fails, but continues through the rest first (so one broken target doesn't hide
failures in the others) — matching how this session's own manual regression-checking already runs one
target after another and reports failures at the end, just scripted instead of typed by hand each time.

## 4. The SDK question needs to stay a deliberate choice, not get automated away

`setup-macos-cross.sh` is the one script in this design that should **not** be fully silent on first run.
The macOS SDK mirror's provenance (README.md's "browser-macos-appkit: building" section) was a real,
discussed decision earlier this session precisely *because* it's a legal gray area, not something to
default into invisibly. A script that silently downloads it the first time anyone runs `build-all.sh`
removes the moment where that tradeoff was actually considered. Concretely: `setup-macos-cross.sh` should
print what it's about to fetch and why (the same explanation README.md already has) and require an explicit
`--yes`/interactive confirmation before the SDK download specifically — everything else in these scripts
(Zig, `cargo-xwin`, apt packages) is unambiguous and fine to automate fully.

This also matters more if a Docker image (§6) is ever built from these scripts: baking the SDK mirror into
a distributable image is a bigger decision than one developer choosing to download it locally, since a
pushed image *is* redistribution at a different scale. Worth treating as a separate decision if §6 is ever
pursued, not something that falls out of "just containerize the scripts."

## 5. Should CI use these scripts too?

Partially, and it's worth being specific about which parts. `windows.yml`/`macos.yml` run on real
Windows/macOS runners that already have their native toolchains — none of `setup-windows-cross.sh`/
`setup-macos-cross.sh` applies there; those scripts are specifically for cross-compiling *from Linux*, which
GitHub's Windows/macOS runners have no need to do. There's no equivalent CI workflow that exercises
`setup-windows-cross.sh`/`setup-macos-cross.sh` today (both cross-compile paths are currently only ever
run manually, on this one dev machine) — that *would* be worth adding: a Linux-runner CI job that runs
`setup-windows-cross.sh`/`setup-macos-cross.sh` from a clean checkout and confirms `build-all.sh` succeeds,
catching the "works on this exact machine" class of bug (the `clang-21`-vs-whatever-version-Ubuntu-ships-
next-year problem §3.2 is designed to route around) before it surfaces as a confusing local failure on a
different box. Not proposed as part of this pass — a natural follow-up once the scripts themselves exist.

## 6. Further out: a Dev Container / Dockerfile

The more complete answer to "prepare the build environment" — one image with every toolchain
(`rustup` + all five targets, `cargo-xwin`, `cargo-zigbuild`, Zig, the macOS SDK, `clang-21`/`lld-21`/
`llvm-21`, GTK/WebKitGTK dev headers) pre-installed, so a fresh contributor runs one `docker build`/`docker
run` (or opens the repo in a [Dev Container](https://containers.dev/)) and has every cross-compile path
available immediately, no scripts to run at all. Deliberately scoped out of this pass: it's a strictly
bigger lift than the scripts above (the scripts are useful with or without it — a Dockerfile would likely
just invoke them), and it sharpens the SDK provenance question in §4 into a real decision about whether
that mirror ends up baked into a shared, potentially-distributed image. Worth doing once the scripts exist
and have been used for a while, not before.

## 7. Suggested order

1. `setup-linux-native.sh` — smallest, lowest-risk, validates the overall script pattern.
2. `setup-windows-cross.sh` — closes the one real *gap* identified in §2.2 (clang-cl/llvm-lib discovery),
   not just a translation of existing README prose.
3. `setup-macos-cross.sh` — same shape as `.cargo/build-macos-appkit.sh`'s existing discovery half, plus
   the interactive SDK-confirmation requirement from §4.
4. `build-all.sh` — orchestrates the three above plus the actual per-target build/cross-compile commands.
5. Revisit §5 (CI coverage for the cross-compile scripts themselves) and §6 (Dev Container) once 1–4 exist
   and have seen real use.
