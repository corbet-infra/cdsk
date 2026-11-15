//! Plans: the commands cdsk would run, and the runners that run them.
//!
//! Every change cdsk makes is an [`Action`], an argv it prints and, only with
//! `--apply`, hands to a [`Runner`]. Planning is therefore pure: a test (or a
//! dry run) inspects the actions and executes nothing.

use cdrv::Event;

use crate::error::{Error, Result};

/// One external command in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// The argv, program first.
    pub argv: Vec<String>,
}

impl Action {
    /// An action from a program and its arguments.
    pub fn new<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
        }
    }

    /// The action as a shell would word it.
    #[must_use]
    pub fn render(&self) -> String {
        self.argv
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

fn quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'_' | b'-' | b'.' | b'/' | b':' | b'=' | b'@' | b'%' | b'+' | b','
                )
        });
    if safe {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Executes one [`Action`].
pub trait Runner {
    /// Run the action, or report why it could not run.
    fn run(&self, action: &Action) -> Result<()>;
}

/// Runs actions as real processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandRunner;

impl Runner for CommandRunner {
    fn run(&self, action: &Action) -> Result<()> {
        let (program, args) = action
            .argv
            .split_first()
            .ok_or_else(|| Error::Refused("an empty action cannot be run".to_owned()))?;
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| Error::command(&action.argv, error.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Command {
                command: action.render(),
                message: stderr.trim().to_owned(),
            });
        }
        Ok(())
    }
}

/// A plan: what cdsk would do, and the events describing why.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// The commands, in the order they must run.
    pub actions: Vec<Action>,
    /// The events the plan accounts for.
    pub events: Vec<Event>,
}

impl Plan {
    /// An empty plan.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one action.
    pub fn push(&mut self, action: Action) {
        self.actions.push(action);
    }

    /// Add one event.
    pub fn emit(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Append another plan, keeping the order.
    pub fn absorb(&mut self, other: Self) {
        self.actions.extend(other.actions);
        self.events.extend(other.events);
    }

    /// Run every action in order through `runner`.
    pub fn run(&self, runner: &dyn Runner) -> Result<()> {
        for action in &self.actions {
            runner.run(action)?;
        }
        Ok(())
    }
}

/// Run a command and return its standard output.
pub(crate) fn capture(argv: &[String]) -> Result<String> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| Error::Refused("an empty command cannot be run".to_owned()))?;
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| Error::command(argv, error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Command {
            command: Action::new(argv.iter().cloned()).render(),
            message: stderr.trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
