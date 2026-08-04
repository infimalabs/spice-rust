use std::fs;

pub(super) fn process_zombie(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat_group_and_state(&stat))
        .is_some_and(|(_, state)| state == b'Z')
}

pub(super) fn process_group_members_terminal(pid: u32) -> bool {
    let Ok(processes) = fs::read_dir("/proc") else {
        return false;
    };
    let mut found = false;
    for process in processes {
        let Ok(process) = process else {
            return false;
        };
        let Some(member) = process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(format!("/proc/{member}/stat")) else {
            continue;
        };
        let Some((group, state)) = stat_group_and_state(&stat) else {
            return false;
        };
        if group == pid {
            found = true;
            if !matches!(state, b'Z' | b'X' | b'x') {
                return false;
            }
        }
    }
    found
}

fn stat_group_and_state(stat: &str) -> Option<(u32, u8)> {
    let (_, fields) = stat.rsplit_once(") ")?;
    let mut fields = fields.split_whitespace();
    let state = *fields.next()?.as_bytes().first()?;
    let _parent = fields.next()?;
    let group = fields.next()?.parse().ok()?;
    Some((group, state))
}
