# tuned-rs

High-performance Rust rewrite of the TuneD system tuning daemon with advanced features beyond the original.

## Features

### Core Compatibility
- Drop-in D-Bus API compatibility with `com.redhat.tuned` / `com.redhat.tuned.control`
- Loads existing profiles from `/usr/lib/tuned` and `/etc/tuned`
- Rollback of original values on profile switch and shutdown (`rollback=auto`)
- PolicyKit authorization matching TuneD (`com.redhat.tuned.<method>` with root fallback)
- SELinux-friendly allowlisted sysfs/proc writes
- Power Profile Daemon (PPD) integration via `tuned-rs-ppd`

### Plugin Coverage

#### Original TuneD Plugins
- **cpu** — governor, energy_performance_preference
- **sysctl** — assignment operators (`>`, `>=`, `=>`, `<`, `<=`, `=<`)
- **vm** — dirty bytes/ratios (including `%`), transparent hugepages
- **disk** — readahead (with `=>` floor semantics), elevator, optional device list
- **acpi** — platform_profile with `|` fallbacks

#### Advanced Plugins (Beyond Original TuneD)
- **network** — TCP/IP stack tuning (congestion control, window scaling, timestamps, SACK, fastopen, buffer sizes)
- **gpu** — AMD GPU power profile management, DRM interface, automatic detection
- **storage** — NVMe APST, I/O scheduler per-device, queue depth optimization
- **thermal** — CPU temperature limits, fan control, thermal policies, trip points
- **battery** — charge thresholds, conservation mode, battery care limits

### Advanced Features
- **Dynamic Tuning** — Real-time workload detection with automatic profile recommendations
- **Performance Telemetry** — Comprehensive metrics collection (CPU, memory, I/O, network, GPU, temperatures, power)
- **Workload Detection** — Intelligent classification (Idle/Light/Moderate/Heavy/Gaming/Compilation)

## Build

```bash
cargo build --release
make check
make test
```

For NixOS:
```bash
nix-shell
cargo build --release
```

## Install

Install conflicts with the Python `tuned` package because both services claim
`com.redhat.tuned` on the system bus.

### From COPR (Fedora, CentOS Stream, RHEL 10 / Rocky 10 / Alma 10)

```bash
sudo dnf copr enable sisyphuscode/tuned-rs
sudo dnf install tuned-rs
```

On RHEL 10 and compatible rebuilds, enable EPEL 10 first if not already enabled.

### From source

```bash
sudo systemctl stop tuned
sudo make install
sudo restorecon -v /usr/sbin/tuned-rs
sudo systemctl enable --now tuned-rs
```

## Verify

```bash
busctl call com.redhat.tuned /Tuned com.redhat.tuned.control profiles
busctl call com.redhat.tuned /Tuned com.redhat.tuned.control active_profile
tuned-adm active
tuned-adm profile balanced
```

## Configuration

### Environment Variables
- `TUNED_RS_PROFILE_DIRS` — comma-separated profile search path
- `TUNED_RS_ROOT` — chroot-style prefix for config/state paths (testing)
- `RUST_LOG` — logging filter, e.g. `RUST_LOG=tuned_rs=debug`

### Configuration Files
- `/etc/tuned/tuned-main.conf` — honors `rollback = auto|not_on_exit`
- `/etc/tuned/ppd.conf` — PPD profile mapping and `sysfs_acpi_monitor`
- `/var/lib/tuned-rs/rollback.json` — persisted rollback state (crash recovery)

### Profile Configuration

Profiles support all original TuneD sections plus new advanced sections:

```ini
[main]
summary=High-performance profile with advanced tuning

[cpu]
governor=performance
energy_performance_preference=performance

[network]
tcp_congestion_control=bbr
tcp_window_scaling=1
tcp_timestamps=1
tcp_sack=1
tcp_fastopen=3

[gpu]
amd_power_profile=high
amd_power_dpm_force_performance_level=high

[storage]
nvme_apst=0
io_scheduler=mq-deadline
nr_requests=256

[thermal]
cpu_temp_limit=85
fan_control=auto

[battery]
charge_start_threshold=20
charge_stop_threshold=80
```

### Power Profile Management

Desktop power mode is controlled by `tuned-rs-ppd` and persisted in
`/etc/tuned/ppd_base_profile`. The underlying TuneD profile name is stored in
`/etc/tuned/active_profile`.

Set the profile through one of these interfaces:

```bash
# Desktop / power-profiles-daemon API
busctl call org.freedesktop.UPower.PowerProfiles /org/freedesktop/UPower/PowerProfiles \
  org.freedesktop.UPower.PowerProfiles SetProfile s performance

# TuneD API
busctl call com.redhat.tuned /Tuned com.redhat.tuned.control switch_profile s throughput-performance b true
```

If the profile keeps reverting to `balanced`, disable automatic ACPI monitoring:

```ini
# /etc/tuned/ppd.conf
sysfs_acpi_monitor=false
```

Then restart `tuned-rs-ppd`.

## SELinux

Label the production binary with `tuned_exec_t` so system `tuned_t` policy applies:

```bash
sudo restorecon -v /usr/sbin/tuned-rs
ps -eZ | grep tuned-rs
```

## Performance Benefits

Compared to the original Python TuneD:
- **Lower memory footprint** — Native Rust binary vs Python interpreter
- **Faster profile switching** — Compiled code with zero-cost abstractions
- **Better concurrency** — Tokio async runtime for non-blocking I/O
- **Enhanced features** — Network, GPU, storage, thermal, and battery plugins
- **Real-time monitoring** — Built-in telemetry and workload detection

## Contributing

Contributions are welcome! Please ensure:
- Code follows Rust best practices and idioms
- All plugins include rollback support
- Changes are tested on target platforms
- Commit messages are clear and descriptive

## License

Same license as original TuneD project.

## Author

Kenny Glowner (@SisyphusCode)
