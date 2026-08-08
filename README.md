# tuned-rs

`tuned-rs` is a native Rust implementation of TuneD for Linux. It installs the
standard `tuned`, `tuned-adm`, and `tuned-ppd` command and service identities,
owns TuneD's system D-Bus API, and consumes existing TuneD profiles.

## Compatibility

- `com.redhat.tuned.control` D-Bus methods, signals, PolicyKit actions, and
  activation identity
- TuneD's optional JSON-RPC Unix socket and signal sockets
- `tuned-adm` profile, verification, plugin, and dynamic-instance commands
- power-profiles-daemon compatibility through `tuned-ppd`
- layered and stacked profiles under `/usr/lib/tuned/profiles` and
  `/etc/tuned/profiles`
- ordered variables, external variable files, nested `${f:...}` functions,
  conditional instances, device matching, and profile-local scripts
- transactional rollback across profile changes, shutdown, and failed applies
- upstream bootloader, CPU, disk, network, scheduler, sysctl, sysfs, VM,
  service, IRQ, USB, video, audio, ACPI, uncore, mount, and realtime controls
- dynamic disk, network, CPU, scheduler, and device-instance tuning

The Arch package provides and replaces both `tuned` and
`power-profiles-daemon`, so it can replace the Python packages without changing
callers or service names.

## Control Center

Launch the interactive processor, network, power-profile, and telemetry UI
from the desktop application menu or a terminal:

```bash
tuned-rs-gui
```

The launcher creates a random loopback-only HTTP endpoint protected by a
192-bit per-session token, opens the default browser, and exits after the tab
has closed. Changes are applied through TuneD's transactional instance API.

## Install

On Ubuntu, install the build dependencies and create a native Debian package:

```bash
sudo apt install build-essential cargo rustc debhelper pkg-config libudev-dev
make check
make test
make deb
sudo apt install ../tuned-rs_0.2.9-1~ppa1~ubuntu26.04.1_$(dpkg --print-architecture).deb
sudo systemctl enable --now tuned.service tuned-ppd.service
```

The Debian package conflicts with and replaces Ubuntu's `tuned`, `tuned-ppd`,
and `power-profiles-daemon` packages because they own the same service and D-Bus
identities. APT will show the replacement transaction before installation.

Published builds are available from the Corinth PPA:

```bash
sudo add-apt-repository ppa:sisyphusaeolides/corinth
sudo apt update
sudo apt install tuned-rs
```

To create a signed, offline-buildable source upload for Launchpad:

```bash
make ppa-source
sudo apt install dput
dput ppa:sisyphusaeolides/corinth \
  ../tuned-rs_0.2.9-1~ppa1~ubuntu26.04.1_source.changes
```

The source target vendors the locked Cargo dependency set into the original
source archive because Launchpad builders do not access crates.io. Use
`make ppa-source-unsigned` only for local source-package validation.

On DNF/RPM based system, add the COPR repo:

```bash
sudo dnf copr enable sisyphuscode/tuned-rs 
sudo dnf install tuned-rs
sudo systemctl enable --now tuned-rs tuned-rs-ppd
```
On an Arch-based system, add the Sisyphus repository to
`/etc/pacman.conf`:

```ini
[sisyphus]
SigLevel = Optional TrustAll
Server = https://sisyphusaeolides.github.io/Sisyphus-Repo/$arch
```

Then install and start TuneD:

```bash
sudo pacman -Syy
sudo pacman -S tuned-rs
sudo systemctl enable --now tuned.service
```

To build from source:

```bash
sudo pacman -S --needed base-devel rust cargo systemd
make check
make test
sudo make install
sudo systemctl enable --now tuned.service
```

## Use

The standard TuneD commands work unchanged:

```bash
tuned-adm list
tuned-adm active
tuned-adm recommend
tuned-adm profile throughput-performance
tuned-adm verify
```

Power-profile-aware desktops can use the standard
`org.freedesktop.UPower.PowerProfiles` interface provided by `tuned-ppd`.

## Configuration

Global settings are read from `/etc/tuned/tuned-main.conf`, including daemon,
dynamic tuning, timing, rollback, profile directories, D-Bus, Unix socket,
instance priority, sysctl reapplication, and startup udev-settle controls.
Power-profile mappings are read from `/etc/tuned/ppd.conf`.

The package installs administrator-editable realtime and CPU-partitioning
variable templates in `/etc/tuned`. Package upgrades preserve local edits.

Useful test-only overrides are:

- `TUNED_RS_ROOT`: prefix absolute system paths with a synthetic root
- `TUNED_RS_PROFILE_DIRS`: override the configured profile search path
- `TUNED_RS_CPUINFO_STRING` and `TUNED_RS_UNAME_STRING`: override conditions
- `RUST_LOG`: select the tracing filter

## Validation

```bash
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
make packaging-check
make proofs-strict
```

The profile integration suite audits the complete bundled upstream profile set
and can audit another TuneD checkout through `TUNED_RS_UPSTREAM_PROFILES`.
Formal models are checked with Fortran, Idris 2, and Agda when those toolchains
are installed.

## License

GPL-2.0-or-later

## Author

Kenny Glowner (SisyphusAeolides)
