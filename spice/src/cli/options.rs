//! One-line help rows for options on first-run command paths.

use clap::Command;

/// One command path, long option name, and its help row.
const OPTIONS: &[(&str, &str, &str)] = &[
    ("", "json", "Answer in machine-readable JSON"),
    (
        "up",
        "foreground",
        "Run the resident attached to this terminal",
    ),
    ("up", "host", "Listen on this IP address"),
    ("up", "port", "Listen on this TCP port"),
    (
        "up",
        "allow-insecure-bind",
        "Confirm that a non-loopback network bind is intentional",
    ),
    (
        "up",
        "auth-token",
        "Require this bearer token from app clients",
    ),
    ("up", "open", "Open the app in the default browser"),
    (
        "install",
        "package",
        "Read the release package at this path",
    ),
    (
        "install",
        "apply",
        "Apply the preview with this exact digest",
    ),
    (
        "install",
        "unapply",
        "Reverse the installed receipt and remove Spice",
    ),
    ("lane add", "team", "Join this existing team ID"),
    ("lane add", "name", "Give the lane this display name"),
    (
        "lane add",
        "life",
        "Set the lane lifecycle to Steer, Drive, or Drain",
    ),
    ("lane add", "vendor", "Launch through this agent vendor"),
    ("lane add", "model", "Launch with this model"),
    ("lane add", "effort", "Launch with this reasoning effort"),
    (
        "task add",
        "pitch",
        "One plain sentence saying why this task matters, at most 160 characters",
    ),
    (
        "task add",
        "description",
        "The detail behind the pitch: what changes and how",
    ),
    (
        "task plan",
        "pitch",
        "One plain sentence saying why this task matters, at most 160 characters",
    ),
    (
        "task design",
        "pitch",
        "One plain sentence saying why this task matters, at most 160 characters",
    ),
    (
        "task capture",
        "pitch",
        "One plain sentence saying why this task matters; derived from the title when absent",
    ),
    (
        "task modify",
        "pitch",
        "One plain sentence saying why this task matters, at most 160 characters",
    ),
    (
        "task modify",
        "description",
        "The detail behind the pitch: what changes and how",
    ),
    (
        "preparation propose",
        "design",
        "Name the held Design when this capability holds several",
    ),
    (
        "preparation propose",
        "recommend",
        "End a pass that built nothing by recommending conclude or collapse",
    ),
    (
        "preparation propose",
        "cite",
        "Add a source, or relabel an assembled one, as RELATION=KEY",
    ),
    (
        "preparation propose",
        "dry-run",
        "Print the assembled proposal and record nothing",
    ),
];

/// Attach every registered option row for `path`, refusing a row that names no
/// option on that exact command. The empty path is the root and owns globals.
pub(super) fn describe(mut command: Command, path: &[&str]) -> Result<Command, String> {
    for &(registered_path, option, help) in OPTIONS {
        if !same_path(registered_path, path) {
            continue;
        }
        if !command
            .get_arguments()
            .any(|argument| argument.get_long() == Some(option))
        {
            return Err(format!(
                "option help for {registered_path:?} names unregistered --{option}"
            ));
        }
        command = command.mut_arg(option, |argument| argument.help(help));
    }
    Ok(command)
}

fn same_path(registered: &str, path: &[&str]) -> bool {
    if registered.is_empty() {
        path.is_empty()
    } else {
        registered.split(' ').eq(path.iter().copied())
    }
}
