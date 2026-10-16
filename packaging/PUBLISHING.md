# Publishing miz

Everything below is done by hand, with your own accounts. The packaging
files in this repository are ready. Each section lists what is still left.

## 0. Before any store

1. Create the public repository `github.com/aj8k/miz` and push `main`.
   The metainfo, the snap metadata and Cargo.toml already point there.
2. Check the release date in `assets/com.antonjoy.miz.metainfo.xml`
   (`<release version="0.1.0" date=...>`), then tag and push:
   `git tag -a v0.1.0 -m "miz 0.1.0" && git push origin v0.1.0`.
   The screenshot URLs use `.../v0.1.0/docs/*.jpg`, so they only resolve
   once the tag exists.
3. For each later release: bump `version` in Cargo.toml, add a
   `<release>` entry to the metainfo, and regenerate
   `packaging/flatpak/cargo-sources.json` if `Cargo.lock` changed.

## 1. .deb (GitHub Releases)

1. `cargo deb` creates `target/debian/miz_0.1.0-1_amd64.deb`.
2. A .deb built on Ubuntu 26.04 needs glibc 2.43, so it installs only on
   26.04 and newer. To support 24.04, build it on 24.04 instead, for example
   in a GitHub Actions `ubuntu-24.04` job.
3. Attach the .deb to the v0.1.0 GitHub release. Users install it with
   `sudo apt install ./miz_0.1.0-1_amd64.deb`. An apt repository or a
   Launchpad PPA are possible later steps; a PPA needs vendored crates,
   because Launchpad builds offline.

## 2. Flathub (`com.antonjoy.miz`)

1. Read <https://docs.flathub.org/docs/for-app-authors/submission>.
2. Fork `github.com/flathub/flathub` and branch from `new-pr`. Add
   `com.antonjoy.miz.yml` and `cargo-sources.json` from `packaging/flatpak/`.
   In that copy, replace the `type: dir` source with:

   ```yaml
   - type: git
     url: https://github.com/aj8k/miz.git
     tag: v0.1.0
     commit: <output of git rev-parse v0.1.0^{commit}>
   ```

   Then open the pull request against `new-pr`.
3. Before submitting, check the build locally:
   `flatpak run org.flatpak.Builder --user --force-clean --repo=packaging/flatpak/repo packaging/flatpak/build <manifest>`
   followed by
   `flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo packaging/flatpak/repo`.
   Today the only error is the screenshots, which need step 0.
4. Expect reviewers to ask about `--filesystem=xdg-run/pipewire-0:ro`. The
   answer: it is the only way for `pw-record` to capture the output monitor,
   it is read-only, no pulseaudio socket is used, and Helvum, coppwr and
   EasyEffects ship the same permission.
5. Once the app is accepted, verify the domain on flathub.org (developer
   portal → the app → Verification). The portal gives you a token to
   publish at `https://antonjoy.com/.well-known/org.flathub.VerifiedApps.txt`.
   The domain must stay yours for as long as the app ID exists.

## 3. Snap Store (`miz`)

1. Create an Ubuntu One account, then run `snapcraft login` and
   `snapcraft register miz`.
2. Build in a clean environment: `snapcraft remote-build` (Launchpad, needs
   a Launchpad account), or `snapcraft pack` with LXD
   (`sudo snap install lxd && sudo lxd init --auto`). A local
   `--destructive-mode` build only works if `rustc`/`cargo` are visible
   to snapcraft. snapd resets `PATH`, so `~/.cargo/bin` isn't seen; for a
   local test, add `rust-channel: none` to a copy of the yaml.
3. Upload with `snapcraft upload --release=edge miz_0.1.0_amd64.snap`. After
   testing, promote it: `snapcraft release miz <revision> stable`.
4. Ask for auto-connection of `pipewire` in the *store-requests*
   category on <https://forum.snapcraft.io> (the Spotify snap has the same
   grant). Points to make:
   - a music visualizer that records only the monitor of the default
     output through PipeWire's `pw-record` (`stream.capture.sink=true`);
   - it never opens a source, plays no audio, and plugs neither
     `audio-record` nor `audio-playback`;
   - `pipewire` is needed because `pw-record` speaks the native protocol,
     and audio-playback/audio-record only grant the pulse socket.

   Until the grant arrives, users have to run `sudo snap connect miz:pipewire`.
   The app prints that hint on stderr, and the store description should
   mention it.

## 4. Local test installs to remove when done

- `flatpak uninstall --user com.antonjoy.miz`
- `sudo snap remove miz mesa-2604`
- `rm -rf ~/.var/app/com.antonjoy.miz ~/snap/miz`
