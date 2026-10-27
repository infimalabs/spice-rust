//! One-line help rows for options on first-run command paths.

use clap::Command;

/// Project guidance shared by every command that names one.
const PROJECT_HELP: &str = "One lowercase word naming the project";

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
        "app",
        "foreground",
        "Attend the window from this terminal until it closes",
    ),
    (
        "app",
        "host",
        "Listen on this IP address when starting the resident",
    ),
    (
        "app",
        "port",
        "Listen on this TCP port when starting the resident",
    ),
    (
        "app",
        "allow-insecure-bind",
        "Confirm that a non-loopback network bind is intentional",
    ),
    (
        "app",
        "auth-token",
        "Require this bearer token from app clients, and present it from the window",
    ),
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
        "lane model",
        "clear",
        "Remove the lane's override and launch with the vendor's model",
    ),
    (
        "lane effort",
        "clear",
        "Remove the lane's override and launch with the vendor's effort",
    ),
    (
        "task add",
        "pitch",
        "One plain sentence saying why this task matters, at most 160 characters",
    ),
    (
        "task add",
        "description",
        "How the work is done: need, mechanism, invariants; the why belongs in --pitch",
    ),
    ("task add", "project", PROJECT_HELP),
    ("task plan", "project", PROJECT_HELP),
    ("task design", "project", PROJECT_HELP),
    ("task capture", "project", PROJECT_HELP),
    ("task modify", "project", PROJECT_HELP),
    ("task oops", "project", PROJECT_HELP),
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
        "How the work is done: need, mechanism, invariants; the why belongs in --pitch",
    ),
    (
        "design propose",
        "design",
        "Name the held Design when this capability holds several",
    ),
    (
        "design propose",
        "recommend",
        "End a pass that built nothing by recommending conclude or collapse",
    ),
    (
        "design propose",
        "cite",
        "Add a source, or relabel an assembled one, as RELATION=KEY",
    ),
    (
        "design propose",
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
