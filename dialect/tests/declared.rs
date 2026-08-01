use dialect::{PROJECT_MAX_DEPTH, PROJECT_MIN_DEPTH};

const DECLARATION: &str = include_str!("../../spice.toml");
const TASKS: &str = "tasks";
const MINIMUM: &str = "project_min_depth";
const MAXIMUM: &str = "project_max_depth";

fn depth(table: &toml::Table, key: &str) -> usize {
    let declared = table
        .get(key)
        .and_then(toml::Value::as_integer)
        .unwrap_or_else(|| panic!("[{TASKS}].{key} must be an integer"));
    usize::try_from(declared).unwrap_or_else(|_| panic!("[{TASKS}].{key} must fit usize"))
}

#[test]
fn compiled_project_depths_equal_their_own_tracked_declarations() {
    let document = DECLARATION
        .parse::<toml::Table>()
        .expect("tracked spice.toml must parse as TOML");
    let tasks = document
        .get(TASKS)
        .and_then(toml::Value::as_table)
        .expect("spice.toml must carry a [tasks] table");

    assert_eq!(PROJECT_MIN_DEPTH, depth(tasks, MINIMUM));
    assert_eq!(PROJECT_MAX_DEPTH, depth(tasks, MAXIMUM));
}
