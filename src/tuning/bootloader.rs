use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::config;
use crate::profile::PluginOptions;
use crate::profile_units::option_value;
use crate::rollback::Rollback;

const BOOT_CMDLINE: &str = "/etc/tuned/bootcmdline";
const DEFAULT_CONTENT: &str =
    "# Managed by tuned-rs.\nTUNED_BOOT_CMDLINE=\nTUNED_BOOT_INITRD_ADD=\n";

pub fn apply_options(rollback: &Rollback, options: &PluginOptions) -> Result<()> {
    let cmdline = effective_cmdline(options)?;
    let initrd = install_initrd_overlay(rollback, options)?;
    if cmdline.is_empty()
        && initrd.is_empty()
        && !options.iter().any(|(name, _)| name.starts_with("cmdline"))
    {
        return Ok(());
    }
    let path = config::resolve_path(BOOT_CMDLINE);
    let current = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DEFAULT_CONTENT.to_string(),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read {}", path.display()))
        }
    };
    let updated = replace_shell_assignment(&current, "TUNED_BOOT_CMDLINE", &cmdline)?;
    let updated = replace_shell_assignment(&updated, "TUNED_BOOT_INITRD_ADD", &initrd)?;
    if updated != current {
        rollback.record_managed_file(&path)?;
        atomic_write(&path, &updated)?;
    }
    if !option_value(options, "skip_grub_config").is_some_and(tuned_bool) {
        if let Some(path) = custom_grub_path(options)? {
            patch_grub_config(rollback, &path, &cmdline, &initrd)?;
        }
        sync_bootloader(&cmdline, &initrd)?;
    }
    Ok(())
}

fn custom_grub_path(options: &PluginOptions) -> Result<Option<PathBuf>> {
    let Some(raw) = option_value(options, "grub2_cfg_file") else {
        return Ok(None);
    };
    let raw = unquote(raw.trim())?;
    if raw.is_empty() {
        return Ok(None);
    }
    let path = Path::new(raw);
    if !path.is_absolute()
        || path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        bail!("grub2_cfg_file must be an absolute normalized path");
    }
    Ok(Some(config::resolve_path_buf(path)))
}

fn patch_grub_config(rollback: &Rollback, path: &Path, cmdline: &str, initrd: &str) -> Result<()> {
    let current = fs::read_to_string(path).with_context(|| {
        format!(
            "Failed to read custom GRUB configuration {}",
            path.display()
        )
    })?;
    let updated = patched_grub_contents(&current, cmdline, initrd);
    if updated != current {
        rollback.record_grub_file(path)?;
        fs::write(path, updated).with_context(|| {
            format!(
                "Failed to patch custom GRUB configuration {}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn patched_grub_contents(contents: &str, cmdline: &str, initrd: &str) -> String {
    const BEGIN: &str = "### BEGIN /etc/grub.d/00_tuned ###";
    const END: &str = "### END /etc/grub.d/00_tuned ###";
    let mut filtered = Vec::new();
    let mut in_tuned = false;
    for line in contents.lines() {
        if line.trim() == BEGIN {
            in_tuned = true;
            continue;
        }
        if in_tuned {
            if line.trim() == END {
                in_tuned = false;
            }
            continue;
        }
        let mut line = line
            .replace(" $tuned_params", "")
            .replace(" $tuned_initrd", "");
        let trimmed = line.trim_start();
        let command = trimmed.split_whitespace().next().unwrap_or_default();
        let rescue = trimmed.contains("rescue");
        if !rescue && matches!(command, "linux" | "linux16" | "linuxefi") {
            line.push_str(" $tuned_params");
        } else if !rescue && matches!(command, "initrd" | "initrd16" | "initrdefi") {
            line.push_str(" $tuned_initrd");
        }
        filtered.push(line);
    }

    let block = [
        BEGIN.to_string(),
        format!("set tuned_params=\"{cmdline}\""),
        format!("set tuned_initrd=\"{initrd}\""),
        END.to_string(),
    ];
    let insertion = filtered
        .iter()
        .position(|line| line.trim_start().starts_with("### END ") && line.contains("/00_header"))
        .map(|index| index + 1)
        .unwrap_or(0);
    filtered.splice(insertion..insertion, block);
    format!("{}\n", filtered.join("\n"))
}

fn install_initrd_overlay(rollback: &Rollback, options: &PluginOptions) -> Result<String> {
    let image = option_value(options, "initrd_add_img").filter(|value| !value.trim().is_empty());
    let directory =
        option_value(options, "initrd_add_dir").filter(|value| !value.trim().is_empty());
    if image.is_some() && directory.is_some() {
        bail!("Only one initrd overlay source may be configured");
    }
    let Some(source) = image.or(directory) else {
        return Ok(String::new());
    };
    let source = config::resolve_path_buf(unquote(source.trim())?);
    let configured_destination = option_value(options, "initrd_dst_img")
        .filter(|value| !value.trim().is_empty())
        .map(str::trim);
    let destination_name = configured_destination
        .map(Path::new)
        .or_else(|| source.file_name().map(Path::new))
        .ok_or_else(|| anyhow::anyhow!("Initrd overlay has no valid filename"))?;
    if destination_name.is_absolute() || destination_name.components().count() != 1 {
        bail!("Invalid initrd overlay filename");
    }
    let destination = config::resolve_path("/boot").join(destination_name);
    rollback.record_boot_file(&destination)?;

    if directory.is_some() {
        validate_overlay_directory(&source)?;
        let temporary = config::resolve_path("/run/tuned/tuned-initrd.tmp");
        if let Some(parent) = temporary.parent() {
            fs::create_dir_all(parent)?;
        }
        let output = fs::File::create(&temporary)?;
        let mut find = Command::new("find")
            .arg(".")
            .current_dir(&source)
            .stdout(Stdio::piped())
            .spawn()
            .context("Failed to start find for initrd overlay")?;
        let status = Command::new("cpio")
            .args(["-o", "-H", "newc"])
            .stdin(Stdio::from(find.stdout.take().unwrap()))
            .stdout(Stdio::from(output))
            .status()
            .context("Failed to start cpio for initrd overlay")?;
        let find_status = find.wait()?;
        if !status.success() || !find_status.success() {
            let _ = fs::remove_file(&temporary);
            bail!("Failed to generate initrd overlay");
        }
        atomic_copy(&temporary, &destination)?;
        fs::remove_file(&temporary)?;
        if option_value(options, "initrd_remove_dir").is_some_and(tuned_bool) {
            fs::remove_dir_all(&source)
                .with_context(|| format!("Failed to remove {}", source.display()))?;
        }
    } else {
        if !source.is_file() {
            bail!("Initrd overlay image is not a file: {}", source.display());
        }
        atomic_copy(&source, &destination)?;
    }
    Ok(format!("/{}", destination_name.to_string_lossy()))
}

fn validate_overlay_directory(path: &Path) -> Result<()> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Invalid initrd overlay directory {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("Initrd overlay source is not a directory");
    }
    let allowed = config::profile_dirs_from_env()
        .into_iter()
        .chain([
            PathBuf::from("/tmp"),
            PathBuf::from("/var/tmp"),
            PathBuf::from("/run/tuned"),
        ])
        .map(config::resolve_path_buf)
        .any(|root| canonical.starts_with(root));
    if !allowed
        || canonical == config::resolve_path("/tmp")
        || canonical == config::resolve_path("/var/tmp")
    {
        bail!("Initrd overlay directory is outside allowed profile and temporary roots");
    }
    if fs::metadata(&canonical)?.uid() != unsafe { libc::geteuid() } {
        bail!("Initrd overlay directory is not owned by the tuned-rs process");
    }
    Ok(())
}

fn atomic_copy(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension("tuned-rs-new");
    fs::copy(source, &temporary)?;
    fs::rename(temporary, destination)?;
    Ok(())
}

pub fn verify_options(options: &PluginOptions, ignore_missing: bool) -> bool {
    let Ok(expected) = effective_cmdline(options) else {
        return false;
    };
    let path = config::resolve_path("/proc/cmdline");
    let Ok(active) = fs::read_to_string(path) else {
        return ignore_missing;
    };
    let active = active.split_whitespace().collect::<Vec<_>>();
    expected
        .split_whitespace()
        .all(|argument| active.contains(&argument))
}

pub fn sync_from_bootcmdline(path: &Path) -> Result<()> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    sync_bootloader(
        shell_assignment(&contents, "TUNED_BOOT_CMDLINE").unwrap_or_default(),
        shell_assignment(&contents, "TUNED_BOOT_INITRD_ADD").unwrap_or_default(),
    )
}

fn effective_cmdline(options: &PluginOptions) -> Result<String> {
    let mut arguments = Vec::<String>::new();
    for (name, raw) in options {
        if !name.starts_with("cmdline") || raw.trim().is_empty() {
            continue;
        }
        let value = unquote(raw.trim())?;
        let (operation, body) = match value.as_bytes().first() {
            Some(b'+') => ('+', value[1..].trim()),
            Some(b'-') => ('-', value[1..].trim()),
            _ => ('+', value),
        };
        for argument in shell_words(body)? {
            if operation == '-' {
                arguments.retain(|existing| existing != &argument);
            } else if !arguments.contains(&argument) {
                arguments.push(argument);
            }
        }
    }
    Ok(arguments.join(" "))
}

fn sync_bootloader(cmdline: &str, initrd: &str) -> Result<()> {
    if std::env::var_os("TUNED_RS_ROOT").is_some() {
        return Ok(());
    }
    match Command::new("grub2-editenv")
        .args([
            "-",
            "set",
            &format!("tuned_params={cmdline}"),
            &format!("tuned_initrd={initrd}"),
        ])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => bail!("grub2-editenv failed with {status}"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    patch_bls_entries()
}

fn patch_bls_entries() -> Result<()> {
    let hook = Path::new("/usr/lib/kernel/install.d/92-tuned.install");
    if !hook.is_file() || !Path::new("/boot/loader/entries").is_dir() {
        return Ok(());
    }
    let machine_id = fs::read_to_string("/etc/machine-id")?;
    let status = Command::new(hook)
        .arg("add")
        .env("KERNEL_INSTALL_MACHINE_ID", machine_id.trim())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        bail!("92-tuned.install failed with {status}")
    }
}

fn replace_shell_assignment(contents: &str, key: &str, value: &str) -> Result<String> {
    if value.contains(['\n', '\r', '\0', '\'', '"', '`', '$']) {
        bail!("Unsafe bootloader value");
    }
    let mut output = Vec::new();
    let mut replaced = false;
    for line in contents.lines() {
        if line
            .trim_start()
            .split_once('=')
            .is_some_and(|(name, _)| name.trim() == key)
        {
            if !replaced {
                output.push(format!("{key}=\"{value}\""));
                replaced = true;
            }
        } else {
            output.push(line.to_string());
        }
    }
    if !replaced {
        output.push(format!("{key}=\"{value}\""));
    }
    Ok(format!("{}\n", output.join("\n")))
}

fn shell_assignment<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    contents.lines().rev().find_map(|line| {
        let (name, value) = line.trim().split_once('=')?;
        (name == key).then_some(value.trim().trim_matches(['\'', '"']))
    })
}

fn unquote(raw: &str) -> Result<&str> {
    if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        Ok(&raw[1..raw.len() - 1])
    } else if raw.starts_with(['\'', '"']) || raw.ends_with(['\'', '"']) {
        bail!("Unbalanced bootloader quotes")
    } else {
        Ok(raw)
    }
}

fn shell_words(raw: &str) -> Result<Vec<String>> {
    if raw.contains(['\n', '\r', '\0', '`', '$', '\'', '"', '\\']) {
        bail!("Bootloader arguments contain unsupported shell syntax");
    }
    Ok(raw.split_whitespace().map(str::to_string).collect())
}

fn tuned_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "y" | "yes" | "true" | "on"
    )
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_named_cmdline_fragments_with_add_remove_semantics() {
        let options = vec![
            ("cmdline_base".to_string(), "+quiet nohz=on".to_string()),
            ("cmdline_child".to_string(), "-quiet".to_string()),
            ("cmdline_last".to_string(), "tsc=reliable".to_string()),
        ];
        assert_eq!(effective_cmdline(&options).unwrap(), "nohz=on tsc=reliable");
    }

    #[test]
    fn shell_assignment_replacement_is_deduplicated_and_safe() {
        let input = "TUNED_BOOT_CMDLINE=old\nX=1\nTUNED_BOOT_CMDLINE=older\n";
        let output = replace_shell_assignment(input, "TUNED_BOOT_CMDLINE", "quiet").unwrap();
        assert_eq!(output.matches("TUNED_BOOT_CMDLINE=").count(), 1);
        assert_eq!(
            shell_assignment(&output, "TUNED_BOOT_CMDLINE"),
            Some("quiet")
        );
        assert!(replace_shell_assignment(input, "TUNED_BOOT_CMDLINE", "$(bad)").is_err());
    }

    #[test]
    fn patches_grub_commands_idempotently_and_preserves_rescue_entries() {
        let grub = "### END /etc/grub.d/00_header ###\nlinux /vmlinuz root=x\ninitrd /initramfs.img\nlinux /vmlinuz-rescue root=x\n";
        let once = patched_grub_contents(grub, "quiet", "/overlay.img");
        let twice = patched_grub_contents(&once, "quiet", "/overlay.img");
        assert_eq!(once, twice);
        assert!(once.contains("set tuned_params=\"quiet\""));
        assert!(once.contains("linux /vmlinuz root=x $tuned_params"));
        assert!(once.contains("initrd /initramfs.img $tuned_initrd"));
        assert!(once.contains("linux /vmlinuz-rescue root=x\n"));
    }
}
