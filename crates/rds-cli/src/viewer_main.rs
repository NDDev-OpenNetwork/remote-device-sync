//! Standalone native application entry point. Uses the local agent and never
//! loads an endpoint key or creates a second network identity.
use clap::{CommandFactory, FromArgMatches, Parser, parser::ValueSource};
use rds_client::local::Client;
use std::{num::NonZeroU32, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Native RDS remote desktop")]
struct Cli {
    target: Option<String>,
    #[arg(long)]
    control_dir: Option<PathBuf>,
    #[arg(long)]
    grant_file: Option<PathBuf>,
    #[command(flatten)]
    options: rds_cli::desktop::Options,
}

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    schema_version: u32,
    target: Option<String>,
    display: Option<u32>,
    max_fps: Option<NonZeroU32>,
    grant_file: Option<PathBuf>,
    resolution: Option<rds_cli::desktop::Resolution>,
    payload_receipts: Option<bool>,
    clipboard: Option<bool>,
    diagnostic_visual_probe: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let matches = Cli::command().get_matches();
    let mut cli = Cli::from_arg_matches(&matches).expect("validated CLI arguments");
    let setup = (|| -> anyhow::Result<_> {
        let (log, directory) = rds_cli::logging::ViewerLog::create()?;
        cli.options.diagnostics_dir = Some(directory);
        Ok(rds_observe::install_with_writer(
            rds_observe::Service::Cli,
            rds_observe::Config::from_env("warn,rds_cli=info,rds_desktop=info")?,
            log,
        )?)
    })();
    match setup {
        Ok(telemetry) => {
            cli.options.diagnostic_health = Some(telemetry.health_observer());
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                tracing::error!(panic=%info,"native viewer panic");
                previous(info);
            }));
            tracing::info!(
                pid = std::process::id(),
                "native viewer started with persistent diagnostics"
            );
            let result = telemetry.run(run(cli, &matches)).await;
            telemetry.finish(result)
        }
        Err(error) => {
            eprintln!("could not initialize viewer diagnostics: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(mut cli: Cli, matches: &clap::ArgMatches) -> anyhow::Result<()> {
    let mut configured_resolution = None;
    let key_path = rds_net::default_key_path();
    let directory = match cli.control_dir.take() {
        Some(directory) => directory,
        None => rds_client::local::control_dir_for_key(
            key_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("no local agent configuration directory"))?,
        )?,
    };
    if let Some(key_path) = key_path {
        let config_path = key_path.with_file_name("viewer.json");
        let read = (|| -> std::io::Result<Vec<u8>> {
            use std::io::Read;
            let file = std::fs::File::from(rustix::fs::open(
                &config_path,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )?);
            if !file.metadata()?.is_file() {
                return Err(std::io::Error::other(
                    "viewer configuration must be a regular file",
                ));
            }
            let mut bytes = Vec::new();
            file.take(16385).read_to_end(&mut bytes)?;
            Ok(bytes)
        })();
        match read {
            Ok(bytes) => {
                anyhow::ensure!(bytes.len() <= 16384, "viewer configuration exceeds 16 KiB");
                let config: Config = serde_json::from_slice(&bytes)?;
                anyhow::ensure!(
                    config.schema_version == 1,
                    "unsupported viewer configuration version"
                );
                if cli.target.is_none() {
                    cli.target = config.target;
                }
                if let Some(display) = config.display
                    && matches.value_source("display") != Some(ValueSource::CommandLine)
                {
                    cli.options.display = display;
                }
                if let Some(fps) = config.max_fps
                    && matches.value_source("max_fps") != Some(ValueSource::CommandLine)
                {
                    cli.options.max_fps = fps;
                }
                if let Some(enabled) = config.payload_receipts
                    && matches.value_source("payload_receipts") != Some(ValueSource::CommandLine)
                {
                    cli.options.payload_receipts = enabled;
                }
                if let Some(enabled) = config.clipboard
                    && matches.value_source("clipboard") != Some(ValueSource::CommandLine)
                {
                    cli.options.clipboard = enabled;
                }
                if let Some(probe) = config.diagnostic_visual_probe
                    && matches.value_source("diagnostic_visual_probe")
                        != Some(ValueSource::CommandLine)
                {
                    cli.options.diagnostic_visual_probe = Some(probe);
                }
                if cli.grant_file.is_none() {
                    cli.grant_file = config.grant_file;
                }
                configured_resolution = config.resolution;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if configure_resolution(&mut cli.options, matches, configured_resolution) {
        let Some(height) = rds_desktop::render::choose_resolution() else {
            return Ok(());
        };
        cli.options.resolution = match height {
            720 => rds_cli::desktop::Resolution::Hd,
            0 => rds_cli::desktop::Resolution::Native,
            _ => rds_cli::desktop::Resolution::FullHd,
        };
    }
    let client = Client::new(directory);
    let grant = rds_cli::desktop::read_grant(cli.grant_file).await?;
    if let Some(target) = cli.target {
        rds_cli::desktop::managed_target(&client, target, grant, cli.options).await
    } else {
        let session = client.selected(None).await?;
        rds_cli::desktop::managed(&client, session, grant, cli.options).await
    }
}

/// Apply explicit quality choices before deciding whether native selection is
/// needed. Command-line values override the persisted profile.
fn configure_resolution(
    options: &mut rds_cli::desktop::Options,
    matches: &clap::ArgMatches,
    configured: Option<rds_cli::desktop::Resolution>,
) -> bool {
    let explicit_cli = matches.value_source("resolution") == Some(ValueSource::CommandLine);
    if !explicit_cli && let Some(resolution) = configured {
        options.resolution = resolution;
    }
    !options.headless && !explicit_cli && configured.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_cli(args: &[&str], configured: Option<&str>) -> (Cli, bool) {
        let matches = Cli::command().try_get_matches_from(args).unwrap();
        let mut cli = Cli::from_arg_matches(&matches).unwrap();
        let config: Config = serde_json::from_str(configured.unwrap_or("{}")).unwrap();
        let picker = configure_resolution(&mut cli.options, &matches, config.resolution);
        (cli, picker)
    }

    #[test]
    fn persisted_quality_is_applied_without_reopening_the_picker() {
        for (profile, height) in [("hd", 720), ("full-hd", 1080), ("native", 0)] {
            let json = format!(r#"{{"resolution":"{profile}"}}"#);
            let (cli, picker) = configured_cli(&["rds-viewer"], Some(&json));
            assert_eq!(cli.options.resolution.height(), height);
            assert!(
                !picker,
                "a persisted profile must not be overwritten by a picker"
            );
        }
    }

    #[test]
    fn command_line_quality_overrides_the_persisted_profile() {
        let (cli, picker) = configured_cli(
            &["rds-viewer", "--resolution", "native"],
            Some(r#"{"resolution":"hd"}"#),
        );
        assert_eq!(cli.options.resolution.height(), 0);
        assert!(!picker);
    }

    #[test]
    fn only_unconfigured_native_launches_need_the_picker() {
        assert!(configured_cli(&["rds-viewer"], None).1);
        assert!(!configured_cli(&["rds-viewer", "--headless"], None).1);
    }
}
