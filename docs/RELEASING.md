# Releasing RustCord

Releases are built and published by GitHub Actions
(`.github/workflows/release.yml`). You never build anything yourself, and it
all works from the GitHub website or the GitHub mobile app.

## Cut a release from your phone

### Option A: Run the workflow (easiest)

1. Open the repo on github.com (in the browser; "Desktop site" helps on a
   phone) or in the GitHub app → **Actions**.
2. Pick **Release** in the workflow list.
3. Tap **Run workflow**. Choose the branch to release from (normally `main`)
   and type the **version**, e.g. `0.2.0`, with no `v` in front.
4. Tap **Run workflow**. It takes roughly 15–25 minutes (the macOS build is the
   slowest).

The workflow creates the tag `v0.2.0` on the latest commit of the branch you
picked and publishes the release.

### Option B: Create a tag

On the website: **Releases → Draft a new release → Choose a tag**, type
`v0.2.0`, pick the target branch, then **Create new tag** and
**Publish release**. The new tag starts the workflow, which attaches the
files to that release when the builds finish. It also replaces the release
text with the download table, so add your own notes afterwards with
**Edit release**. Until the builds finish, the release has no downloads,
which is why Option A is simpler. From a computer:
`git tag v0.2.0 && git push origin v0.2.0`.

### Versions

- `0.2.0`: a normal release, marked **Latest**.
- Anything with a `-`, like `0.2.0-beta.1` or `0.3.0-rc.1`: published as a
  **pre-release**. It's not marked Latest, so the "latest" download link
  keeps pointing at the last normal release.
- The version doesn't need to match `Cargo.toml`. CI writes the release
  version into the build, so Settings → About, the Windows file properties
  and the macOS app info all show it. Bump `Cargo.toml` in a normal PR
  whenever it suits you.
- If a run fails, fix the problem and run it again with the same version. It
  reuses the tag and the release and replaces the files. To start over,
  delete the release and the tag on GitHub first.

## What gets built

| File | What it is |
| --- | --- |
| `RustCord-<v>-windows-x86_64-setup.exe` | Windows installer (Inno Setup). Installs for the current user (no admin), Start-menu shortcut, optional desktop shortcut, uninstaller in Settings → Apps. |
| `RustCord-<v>-windows-x86_64.zip` | Portable Windows build: `rustcord.exe`, `LICENSE.txt`, `README.md`. |
| `RustCord-<v>-macos-universal.zip` | `RustCord.app` for Apple Silicon and Intel (macOS 11+), signed ad hoc. |
| `RustCord-<v>-linux-x86_64.tar.gz` | `rustcord`, `install.sh` (adds a menu entry under `~/.local`), `.desktop` file, icon, LICENSE, README. Built on Ubuntu 22.04 so it runs on most current distros. |
| `SHA256SUMS.txt` | SHA-256 checksums of all of the above. |

Only the app and its licence and readme are shipped. `examples/` (themes,
plugins) is never packaged.

The Windows `.exe` has the RustCord icon and version info built in
(`build.rs` + `winresource`) and opens without a console window. Debug
builds (`cargo run`) still have one, so logs stay visible.

## Where the downloads appear

- **Releases** on the repo's main page (right-hand side, or below the README
  on a phone), or directly at
  `https://github.com/KeoLogs/rustcord/releases/latest`.
- Every release starts with a download table, followed by GitHub's
  generated "What's Changed" list of merged PRs since the previous tag.
- While a run is in progress, you can follow it under **Actions → Release**.
  If any build fails, nothing is published.

## First launch on unsigned builds

The builds are not code-signed (that needs paid certificates):

- **Windows**: SmartScreen may show "Windows protected your PC". Tap
  **More info → Run anyway**. This happens once per new version.
- **macOS**: the first time, right-click (Control-click) `RustCord.app` →
  **Open** → **Open**. On macOS 15 and later, open it once, then go to
  **System Settings → Privacy & Security** and click **Open Anyway**.
- **Linux**: no prompt. `chmod +x` is already set inside the archive.

## Building the installer by hand (Windows)

Install [Inno Setup 6](https://jrsoftware.org/isinfo.php), then from the repo
root:

```bat
cargo build --release
"C:\Program Files (x86)\Inno Setup 6\ISCC.exe" /DAppVersion=0.1.0 /DSourceDir=%CD% /DOutputDir=%CD%\dist packaging\windows\rustcord.iss
```

## The icon

`assets/icon/rustcord.svg` is the source. After editing it, run
`python3 assets/icon/generate.py` (needs `rsvg-convert` and Pillow) and commit
the regenerated `png/`, `rustcord.ico` and `rustcord.icns`. The window icon
uses `png/rustcord-256.png`, the Windows exe uses `rustcord.ico` and the macOS
app uses `rustcord.icns`.
