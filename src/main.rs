use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[command(
    name = "ivm",
    version,
    about = "Manage pinned Istio clients and profiles"
)]
struct Cli {
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Subcommand)]
enum CommandKind {
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    Status,
    Install,
    Apply(ApplyArgs),
    Unapply,
    Uninstall,
}

#[derive(Args)]
struct ApplyArgs {
    #[arg(short = 'd', long = "delete")]
    delete: bool,
}

#[derive(Subcommand)]
enum ProfileCommand {
    List,
    Use { name: String },
    Set(ProfileSet),
}

#[derive(Args)]
struct ProfileSet {
    name: String,
    #[arg(long)]
    context: String,
    #[arg(long)]
    istio_version: String,
    #[arg(long = "set", value_name = "KEY=VALUE")]
    sets: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Config {
    active_profile: Option<String>,
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
}

#[derive(Serialize, Deserialize)]
struct Profile {
    context: String,
    istio_version: String,
    #[serde(default)]
    sets: Vec<String>,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        CommandKind::Profile { command } => profile_command(command),
        CommandKind::Status => status(),
        CommandKind::Install => install(),
        CommandKind::Apply(args) => apply(if args.delete { "uninstall" } else { "install" }),
        CommandKind::Unapply => apply("uninstall"),
        CommandKind::Uninstall => uninstall(),
    }
}

fn profile_command(command: ProfileCommand) -> Result<()> {
    let mut config = load_config()?;
    match command {
        ProfileCommand::List => {
            for (name, profile) in &config.profiles {
                let marker = if config.active_profile.as_deref() == Some(name) {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{marker} {name}: {} ({})",
                    profile.istio_version, profile.context
                );
            }
        }
        ProfileCommand::Use { name } => {
            if !config.profiles.contains_key(&name) {
                bail!("profile not found: {name}");
            }
            config.active_profile = Some(name.clone());
            save_config(&config)?;
            println!("active profile: {name}");
        }
        ProfileCommand::Set(ProfileSet {
            name,
            context,
            istio_version,
            sets,
        }) => {
            validate_version(&istio_version)?;
            config.profiles.insert(
                name.clone(),
                Profile {
                    context,
                    istio_version,
                    sets,
                },
            );
            if config.active_profile.is_none() {
                config.active_profile = Some(name.clone());
            }
            save_config(&config)?;
            println!("saved profile: {name}");
        }
    }
    Ok(())
}

fn status() -> Result<()> {
    let config = load_config()?;
    let (name, profile) = active_profile(&config)?;
    let binary = istioctl_path(&profile.istio_version)?;
    println!("profile: {name}");
    println!("context: {}", profile.context);
    println!("istio: {}", profile.istio_version);
    println!("istioctl: {}", binary.display());
    println!("installed: {}", binary.is_file());
    Ok(())
}

fn install() -> Result<()> {
    let config = load_config()?;
    let (_, profile) = active_profile(&config)?;
    let binary = istioctl_path(&profile.istio_version)?;
    if binary.is_file() {
        println!("istioctl {} already installed", profile.istio_version);
        return Ok(());
    }
    download_istioctl(&profile.istio_version, &binary)
}

fn uninstall() -> Result<()> {
    let config = load_config()?;
    let (_, profile) = active_profile(&config)?;
    let binary = istioctl_path(&profile.istio_version)?;
    let version_dir = binary.parent().context("invalid istioctl cache path")?;
    if version_dir.is_dir() {
        fs::remove_dir_all(version_dir).context("removing cached istioctl")?;
        println!("removed cached Istio {}", profile.istio_version);
    } else {
        println!("Istio {} is not cached", profile.istio_version);
    }
    Ok(())
}

fn apply(action: &str) -> Result<()> {
    let config = load_config()?;
    let (_, profile) = active_profile(&config)?;
    let binary = istioctl_path(&profile.istio_version)?;
    if !binary.is_file() {
        bail!(
            "istioctl {} is not installed; run `ivm install` first",
            profile.istio_version
        );
    }

    let mut command = Command::new(binary);
    command.arg(action).arg("--context").arg(&profile.context);
    if action == "install" {
        command.arg("-y");
    } else {
        command.args(["--purge", "-y"]);
    }
    if action == "install" {
        for value in &profile.sets {
            command.args(["--set", value]);
        }
    }
    let status = command.status().context("running istioctl")?;
    if !status.success() {
        bail!("istioctl {action} failed with {status}");
    }
    Ok(())
}

fn download_istioctl(version: &str, destination: &Path) -> Result<()> {
    let target = target_name()?;
    let url = format!(
        "https://github.com/istio/istio/releases/download/{version}/istio-{version}-{target}.tar.gz"
    );
    let temp = env::temp_dir().join(format!("ivm-{version}-{}", std::process::id()));
    fs::create_dir_all(&temp).context("creating temporary download directory")?;
    fs::create_dir_all(destination.parent().unwrap())?;

    let mut curl = Command::new("curl")
        .args(["-fL", &url])
        .stdout(Stdio::piped())
        .spawn()
        .context("starting curl")?;
    let tar_status = Command::new("tar")
        .args(["-xz", "-C"])
        .arg(&temp)
        .stdin(curl.stdout.take().unwrap())
        .status()
        .context("starting tar")?;
    let curl_status = curl.wait().context("waiting for curl")?;
    if !curl_status.success() || !tar_status.success() {
        let _ = fs::remove_dir_all(&temp);
        bail!("could not download Istio {version} from {url}");
    }

    let extracted = temp.join(format!("istio-{version}")).join("bin/istioctl");
    if !extracted.is_file() {
        let _ = fs::remove_dir_all(&temp);
        bail!("downloaded archive did not contain istioctl");
    }
    fs::copy(&extracted, destination).context("installing istioctl")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;
    }
    let _ = fs::remove_dir_all(&temp);
    println!("installed Istio {version}: {}", destination.display());
    Ok(())
}

fn load_config() -> Result<Config> {
    let path = config_path()?;
    if !path.is_file() {
        return Ok(Config::default());
    }
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}

fn save_config(config: &Config) -> Result<()> {
    let path = config_path()?;
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, toml::to_string_pretty(config)?)?;
    Ok(())
}

fn config_path() -> Result<PathBuf> {
    if let Ok(path) = env::var("IVM_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/ivm/config.toml"))
}

fn active_profile(config: &Config) -> Result<(String, &Profile)> {
    let name = config
        .active_profile
        .clone()
        .context("no active profile; use `ivm profile set`")?;
    let profile = config
        .profiles
        .get(&name)
        .context("active profile is missing")?;
    Ok((name, profile))
}

fn istioctl_path(version: &str) -> Result<PathBuf> {
    validate_version(version)?;
    let home = env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".local/share/ivm/istio")
        .join(version)
        .join("istioctl"))
}

fn validate_version(version: &str) -> Result<()> {
    if version.is_empty()
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
    {
        bail!("invalid Istio version: {version}");
    }
    Ok(())
}

fn target_name() -> Result<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Ok("osx-arm64"),
        ("macos", "x86_64") => Ok("osx-amd64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        ("linux", "aarch64") => Ok("linux-arm64v8"),
        _ => bail!(
            "unsupported platform: {}-{}",
            env::consts::OS,
            env::consts::ARCH
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_injection_in_version() {
        assert!(validate_version("1.22.1/../../tmp").is_err());
    }
}
