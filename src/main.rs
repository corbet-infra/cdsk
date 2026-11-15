//! The `cdsk` command: print a plan; execute it only with `--apply`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cdrv::Timestamp;
use clap::{Parser, Subcommand};

use cdsk::disks::{CLEAN, plan_reset};
use cdsk::fixture::{FixtureFs, FixtureState, FixtureZfs};
use cdsk::plan::{CommandRunner, Plan};
use cdsk::{Declaration, Error, Result};

#[derive(Debug, Parser)]
#[command(
    name = "cdsk",
    version,
    about = "Storage convergence and instance disks for the VM/GPU system"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print a plan; nothing runs without --apply.
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
}

#[derive(Debug, Subcommand)]
enum PlanCommand {
    /// Converge dataset shape and ownership from a declaration.
    Converge {
        /// The nixstorage-style declaration, as JSON.
        #[arg(long)]
        decl: PathBuf,
        /// Recorded zfs and filesystem observations.
        #[arg(long)]
        state: PathBuf,
        /// Execute the plan. Without it, only the commands are printed.
        #[arg(long)]
        apply: bool,
    },
    /// Plan a reset of an instance disk to its @clean snapshot.
    Reset {
        /// The instance whose disk to reset.
        instance: String,
        /// Recorded observations and the instance disk binding.
        #[arg(long)]
        state: PathBuf,
        /// Execute the plan. Without it, only the commands are printed.
        #[arg(long)]
        apply: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cdsk: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Plan { command } => match command {
            PlanCommand::Converge { decl, state, apply } => plan_converge(&decl, &state, apply),
            PlanCommand::Reset {
                instance,
                state,
                apply,
            } => plan_reset_command(&instance, &state, apply),
        },
    }
}

fn plan_converge(decl_path: &Path, state_path: &Path, apply: bool) -> Result<()> {
    let decl: Declaration = read_json(decl_path, "declaration")?;
    let state: FixtureState = read_json(state_path, "state")?;
    let zfs = FixtureZfs::new(&state);
    let fs = FixtureFs::new(&state);
    let converged = cdsk::converge(&decl, &zfs, &fs, &hostname(), now())?;
    print_plan(&converged.plan, apply)
}

fn plan_reset_command(instance: &str, state_path: &Path, apply: bool) -> Result<()> {
    let state: FixtureState = read_json(state_path, "state")?;
    let disk = state
        .instances
        .get(instance)
        .cloned()
        .ok_or_else(|| Error::UnknownInstance(instance.to_owned()))?;
    let zfs = FixtureZfs::new(&state);
    let fs = FixtureFs::new(&state);
    let mut log = cdrv::EventLog::new(hostname(), cdrv::Source::Cdsk, 1);
    let plan = plan_reset(&disk, CLEAN, &zfs, &fs, &mut log, now())?;
    print_plan(&plan, apply)
}

fn print_plan(plan: &Plan, apply: bool) -> Result<()> {
    for action in &plan.actions {
        println!("{}", action.render());
    }
    for event in &plan.events {
        let line = serde_json::to_string(event).map_err(|error| Error::Json {
            what: "event".to_owned(),
            source: error,
        })?;
        eprintln!("{line}");
    }
    if apply {
        plan.run(&CommandRunner)?;
        eprintln!("cdsk: applied {} action(s)", plan.actions.len());
    } else {
        eprintln!(
            "cdsk: {} action(s) planned; nothing runs without --apply",
            plan.actions.len()
        );
    }
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|error| Error::Io {
        path: path.to_path_buf(),
        source: error,
    })?;
    serde_json::from_str(&text).map_err(|error| Error::Json {
        what: format!("{what} {}", path.display()),
        source: error,
    })
}

fn now() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    Timestamp::from_millis(millis)
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "localhost".to_owned())
}
