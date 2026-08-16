use std::collections::{HashMap, VecDeque};
use std::process::ExitCode;

use anyhow::{Context, Result};
use zbus::proxy;

const LOG_FILE: &str = "/var/log/tuned/tuned.log";

type InstanceSummary = (String, String);
type InstanceListReply = (bool, String, Vec<InstanceSummary>);

#[proxy(
    interface = "com.redhat.tuned.control",
    default_service = "com.redhat.tuned",
    default_path = "/Tuned"
)]
trait Tuned {
    #[zbus(name = "active_profile")]
    fn active_profile(&self) -> zbus::Result<String>;

    #[zbus(name = "post_loaded_profile")]
    fn post_loaded_profile(&self) -> zbus::Result<String>;

    #[zbus(name = "profile_mode")]
    fn profile_mode(&self) -> zbus::Result<(String, String)>;

    #[zbus(name = "profiles")]
    fn profiles(&self) -> zbus::Result<Vec<String>>;

    #[zbus(name = "profiles2")]
    fn profiles2(&self) -> zbus::Result<Vec<(String, String)>>;

    #[zbus(name = "profile_info")]
    fn profile_info(&self, profile_name: &str) -> zbus::Result<(bool, String, String, String)>;

    #[zbus(name = "recommend_profile")]
    fn recommend_profile(&self) -> zbus::Result<String>;

    #[zbus(name = "switch_profile")]
    fn switch_profile(&self, profile_name: &str) -> zbus::Result<(bool, String)>;

    #[zbus(name = "auto_profile")]
    fn auto_profile(&self) -> zbus::Result<(bool, String)>;

    #[zbus(name = "disable")]
    fn disable(&self) -> zbus::Result<bool>;

    #[zbus(name = "verify_profile")]
    fn verify_profile(&self) -> zbus::Result<bool>;

    #[zbus(name = "verify_profile_ignore_missing")]
    fn verify_profile_ignore_missing(&self) -> zbus::Result<bool>;

    #[zbus(name = "get_all_plugins")]
    fn get_all_plugins(&self) -> zbus::Result<HashMap<String, HashMap<String, String>>>;

    #[zbus(name = "get_plugin_hints")]
    fn get_plugin_hints(&self, plugin_name: &str) -> zbus::Result<HashMap<String, String>>;

    #[zbus(name = "instance_acquire_devices")]
    fn instance_acquire_devices(
        &self,
        devices: &str,
        instance_name: &str,
    ) -> zbus::Result<(bool, String)>;

    #[zbus(name = "get_instances")]
    fn get_instances(&self, plugin_name: &str) -> zbus::Result<InstanceListReply>;

    #[zbus(name = "instance_get_devices")]
    fn instance_get_devices(
        &self,
        instance_name: &str,
    ) -> zbus::Result<(bool, String, Vec<String>)>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    List { choice: ListChoice, verbose: bool },
    Active,
    Off,
    Profile(Vec<String>),
    ProfileInfo(String),
    Recommend,
    Verify { ignore_missing: bool },
    AutoProfile,
    ProfileMode,
    InstanceAcquireDevices { devices: String, instance: String },
    GetInstances(String),
    InstanceGetDevices(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListChoice {
    Profiles,
    Plugins,
}

#[tokio::main]
async fn main() -> ExitCode {
    let command = match parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            return ExitCode::from(1);
        }
    };

    match command {
        Command::Help => {
            print_usage();
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("tuned-adm {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        command => match run(command).await {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(1),
            Err(error) => {
                eprintln!("{error:#}");
                ExitCode::from(3)
            }
        },
    }
}

async fn run(command: Command) -> Result<bool> {
    let connection = zbus::Connection::system()
        .await
        .context("Unable to connect to the system D-Bus")?;
    let tuned = TunedProxy::new(&connection)
        .await
        .context("TuneD is not available on D-Bus; ensure tuned.service is running")?;

    match command {
        Command::List { choice, verbose } => match choice {
            ListChoice::Profiles => print_profiles(&tuned).await,
            ListChoice::Plugins => print_plugins(&tuned, verbose).await,
        },
        Command::Active => print_active(&tuned).await,
        Command::Off => {
            let disabled = tuned.disable().await?;
            if !disabled {
                eprintln!("Cannot disable active profile.");
            }
            Ok(disabled)
        }
        Command::Profile(profiles) => {
            if profiles.is_empty() {
                return print_profiles(&tuned).await;
            }
            let profile = profiles.join(" ");
            let (success, message) = tuned.switch_profile(&profile).await?;
            if !success {
                eprintln!("Unable to switch profile: {message}");
            }
            Ok(success)
        }
        Command::ProfileInfo(mut profile) => {
            if profile.is_empty() {
                profile = tuned.active_profile().await?;
            }
            if profile.is_empty() {
                println!("No current active profile.");
                return Ok(false);
            }
            let info = tuned.profile_info(&profile).await?;
            if !info.0 {
                println!("Unable to get information about profile '{profile}'");
                return Ok(false);
            }
            println!("Profile name:\n{}\n", info.1);
            println!("Profile summary:\n{}\n", info.2);
            println!("Profile description:\n{}", info.3);
            Ok(true)
        }
        Command::Recommend => {
            println!("{}", tuned.recommend_profile().await?);
            Ok(true)
        }
        Command::Verify { ignore_missing } => {
            let verified = if ignore_missing {
                tuned.verify_profile_ignore_missing().await?
            } else {
                tuned.verify_profile().await?
            };
            if verified {
                println!(
                    "Verification succeeded, current system settings match the preset profile."
                );
            } else {
                println!(
                    "Verification failed, current system settings differ from the preset profile."
                );
                println!("You can mostly fix this by restarting the TuneD daemon, e.g.:");
                println!("  systemctl restart tuned");
                println!("or");
                println!("  service tuned restart");
                println!("Sometimes (if some plugins like bootloader are used) a reboot may be required.");
            }
            println!("See TuneD log file ('{LOG_FILE}') for details.");
            Ok(verified)
        }
        Command::AutoProfile => {
            let (success, message) = tuned.auto_profile().await?;
            if !success {
                eprintln!("Unable to switch profile: {message}");
            }
            Ok(success)
        }
        Command::ProfileMode => {
            let (mode, error) = tuned.profile_mode().await?;
            println!("Profile selection mode: {mode}");
            if !error.is_empty() {
                eprintln!("{error}");
            }
            Ok(error.is_empty())
        }
        Command::InstanceAcquireDevices { devices, instance } => {
            let (success, message) = tuned.instance_acquire_devices(&devices, &instance).await?;
            if !success {
                eprintln!("Unable to acquire devices: {message}");
            }
            Ok(success)
        }
        Command::GetInstances(plugin) => {
            let (success, message, instances) = tuned.get_instances(&plugin).await?;
            if !success {
                eprintln!("Unable to list instances: {message}");
                return Ok(false);
            }
            for (instance, plugin) in instances {
                println!("{instance} ({plugin})");
            }
            Ok(true)
        }
        Command::InstanceGetDevices(instance) => {
            let (success, message, devices) = tuned.instance_get_devices(&instance).await?;
            if !success {
                eprintln!("Unable to list devices: {message}");
                return Ok(false);
            }
            for device in devices {
                println!("{device}");
            }
            Ok(true)
        }
        Command::Help | Command::Version => unreachable!("handled before D-Bus connection"),
    }
}

async fn print_profiles(tuned: &TunedProxy<'_>) -> Result<bool> {
    let profiles = match tuned.profiles2().await {
        Ok(profiles) => profiles,
        Err(_) => tuned
            .profiles()
            .await?
            .into_iter()
            .map(|profile| (profile, String::new()))
            .collect(),
    };

    println!("Available profiles:");
    for (profile, summary) in profiles {
        println!("{}", format_profile_line(&profile, &summary));
    }
    print_active(tuned).await
}

async fn print_active(tuned: &TunedProxy<'_>) -> Result<bool> {
    let profile = tuned.active_profile().await?;
    if profile.is_empty() {
        println!("No current active profile.");
        return Ok(false);
    }
    println!("Current active profile: {profile}");

    let post_loaded = tuned.post_loaded_profile().await?;
    if !post_loaded.is_empty() && post_loaded != profile {
        println!("Current post-loaded profile: {post_loaded}");
    }
    Ok(true)
}

async fn print_plugins(tuned: &TunedProxy<'_>, verbose: bool) -> Result<bool> {
    let plugins = tuned.get_all_plugins().await?;
    let mut names = plugins.keys().cloned().collect::<Vec<_>>();
    names.sort_unstable();

    for name in names {
        println!("{name}");
        if !verbose {
            continue;
        }
        let hints = tuned.get_plugin_hints(&name).await?;
        let mut options = plugins[&name].keys().cloned().collect::<Vec<_>>();
        options.sort_unstable();
        for option in options {
            println!("\t{option}");
            if let Some(hint) = hints.get(&option).filter(|hint| !hint.is_empty()) {
                println!("\t\t{hint}");
            }
        }
    }
    Ok(true)
}

fn format_profile_line(profile: &str, summary: &str) -> String {
    let prefix = format!("- {profile}");
    if summary.is_empty() {
        return prefix;
    }
    let padding = 30usize.saturating_sub(prefix.chars().count()).max(1);
    format!("{prefix}{}- {summary}", " ".repeat(padding))
}

fn parse_args(args: impl Iterator<Item = String>) -> std::result::Result<Command, String> {
    let mut args = args.collect::<VecDeque<_>>();

    loop {
        let Some(argument) = args.front().map(String::as_str) else {
            return Err("a command is required".to_string());
        };
        match argument {
            "--help" | "-h" => return Ok(Command::Help),
            "--version" | "-v" => return Ok(Command::Version),
            "--debug" | "-d" | "--async" | "-a" => {
                args.pop_front();
            }
            "--timeout" | "-t" => {
                args.pop_front();
                let value = args
                    .pop_front()
                    .ok_or_else(|| "--timeout requires a positive integer".to_string())?;
                if value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .is_none()
                {
                    return Err(format!("{value} has to be > 0"));
                }
            }
            "--loglevel" | "-l" => {
                args.pop_front();
                if args.pop_front().is_none() {
                    return Err("--loglevel requires a value".to_string());
                }
            }
            _ if argument.starts_with("--timeout=") => {
                let value = argument.trim_start_matches("--timeout=");
                if value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .is_none()
                {
                    return Err(format!("{value} has to be > 0"));
                }
                args.pop_front();
            }
            _ if argument.starts_with("--loglevel=") => {
                args.pop_front();
            }
            _ if argument.starts_with('-') => {
                return Err(format!("unrecognized option '{argument}'"));
            }
            _ => break,
        }
    }

    let command = args.pop_front().expect("checked above");
    match command.as_str() {
        "list" => parse_list(args),
        "active" => expect_empty(args, Command::Active),
        "off" => expect_empty(args, Command::Off),
        "profile" => Ok(Command::Profile(args.into_iter().collect())),
        "profile_info" => match args.len() {
            0 => Ok(Command::ProfileInfo(String::new())),
            1 => Ok(Command::ProfileInfo(
                args.pop_front().expect("length checked"),
            )),
            _ => Err("profile_info accepts at most one profile".to_string()),
        },
        "recommend" => expect_empty(args, Command::Recommend),
        "verify" => parse_verify(args),
        "auto_profile" => expect_empty(args, Command::AutoProfile),
        "profile_mode" => expect_empty(args, Command::ProfileMode),
        "instance_acquire_devices" => {
            if args.len() != 2 {
                return Err("instance_acquire_devices requires devices and instance".to_string());
            }
            Ok(Command::InstanceAcquireDevices {
                devices: args.pop_front().expect("length checked"),
                instance: args.pop_front().expect("length checked"),
            })
        }
        "get_instances" => match args.len() {
            0 => Ok(Command::GetInstances(String::new())),
            1 => Ok(Command::GetInstances(
                args.pop_front().expect("length checked"),
            )),
            _ => Err("get_instances accepts at most one plugin name".to_string()),
        },
        "instance_get_devices" => {
            if args.len() != 1 {
                return Err("instance_get_devices requires an instance name".to_string());
            }
            Ok(Command::InstanceGetDevices(
                args.pop_front().expect("length checked"),
            ))
        }
        _ => Err(format!("unknown command '{command}'")),
    }
}

fn parse_list(mut args: VecDeque<String>) -> std::result::Result<Command, String> {
    let mut choice = ListChoice::Profiles;
    let mut verbose = false;
    while let Some(argument) = args.pop_front() {
        match argument.as_str() {
            "profiles" => choice = ListChoice::Profiles,
            "plugins" => choice = ListChoice::Plugins,
            "--verbose" | "-v" => verbose = true,
            _ => return Err(format!("invalid list argument '{argument}'")),
        }
    }
    Ok(Command::List { choice, verbose })
}

fn parse_verify(mut args: VecDeque<String>) -> std::result::Result<Command, String> {
    let mut ignore_missing = false;
    while let Some(argument) = args.pop_front() {
        match argument.as_str() {
            "--ignore-missing" | "-i" => ignore_missing = true,
            _ => return Err(format!("invalid verify argument '{argument}'")),
        }
    }
    Ok(Command::Verify { ignore_missing })
}

fn expect_empty(args: VecDeque<String>, command: Command) -> std::result::Result<Command, String> {
    if args.is_empty() {
        Ok(command)
    } else {
        Err("this command does not accept positional arguments".to_string())
    }
}

fn print_usage() {
    eprintln!("usage: tuned-adm [options] COMMAND [arguments]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  list [plugins|profiles] [-v|--verbose]");
    eprintln!("  active");
    eprintln!("  off");
    eprintln!("  profile [PROFILE ...]");
    eprintln!("  profile_info [PROFILE]");
    eprintln!("  recommend");
    eprintln!("  verify [-i|--ignore-missing]");
    eprintln!("  auto_profile");
    eprintln!("  profile_mode");
    eprintln!("  instance_acquire_devices DEVICES INSTANCE");
    eprintln!("  get_instances [PLUGIN]");
    eprintln!("  instance_get_devices INSTANCE");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Command {
        parse_args(arguments.iter().map(|argument| (*argument).to_string())).unwrap()
    }

    #[test]
    fn parses_verbose_plugin_listing() {
        assert_eq!(
            parse(&["list", "plugins", "--verbose"]),
            Command::List {
                choice: ListChoice::Plugins,
                verbose: true,
            }
        );
    }

    #[test]
    fn preserves_stacked_profile_order() {
        assert_eq!(
            parse(&["profile", "balanced", "throughput-performance"]),
            Command::Profile(vec![
                "balanced".to_string(),
                "throughput-performance".to_string(),
            ])
        );
    }

    #[test]
    fn formats_profile_summary_at_upstream_column() {
        assert_eq!(
            format_profile_line("balanced", "General use"),
            "- balanced                    - General use"
        );
    }

    #[test]
    fn rejects_non_positive_timeout() {
        assert!(parse_args(["--timeout", "0", "active"].into_iter().map(str::to_string)).is_err());
    }
}
