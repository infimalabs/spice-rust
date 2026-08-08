#[cfg(unix)]
mod command_tests {
    use std::os::unix::process::CommandExt as _;
    use std::sync::{Arc, Mutex};

    use super::*;

    fn spawn_process(
        program: &str,
        args: &[&str],
        piped_output: bool,
        probe: impl FnMut(GroupProbeSite, rustix::process::Pid) -> GroupPresence + 'static,
    ) -> GitProcess {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(if piped_output {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(if piped_output {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .process_group(0);
        GitProcess::with_group_probe(
            command.spawn().expect("launch process-group fixture"),
            probe,
        )
    }

    fn supervised_process(
        probe: impl FnMut(GroupProbeSite, rustix::process::Pid) -> GroupPresence + 'static,
    ) -> (GitProcess, CommandWorkers) {
        let mut process = spawn_process("git", &["--version"], true, probe);
        let workers =
            CommandWorkers::start(&mut process, None).expect("start fixture output workers");
        (process, workers)
    }

    #[test]
    fn every_kernel_probe_answer_keeps_its_own_evidence_state() {
        assert_eq!(classify_group_probe(Ok(())), GroupPresence::Present);
        assert_eq!(
            classify_group_probe(Err(rustix::io::Errno::PERM)),
            GroupPresence::Present,
        );
        assert_eq!(
            classify_group_probe(Err(rustix::io::Errno::SRCH)),
            GroupPresence::Absent,
        );
        assert_eq!(
            classify_group_probe(Err(rustix::io::Errno::INVAL)),
            GroupPresence::Indeterminate,
        );
    }

    #[test]
    fn supervise_git_forgives_an_unreadable_group_but_faults_an_observed_survivor() {
        let unread_sites = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&unread_sites);
        let (mut process, workers) = supervised_process(move |site, _| {
            observed
                .lock()
                .expect("probe log is not poisoned")
                .push(site);
            GroupPresence::Indeterminate
        });

        let (status, _, _) = supervise_git(
            &mut process,
            workers,
            &["--version"],
            LOCAL_COMMAND_LIMIT,
            None,
        )
        .expect("an unreadable post-exit probe is not a survivor");
        assert!(status.success());
        assert_eq!(
            &*unread_sites.lock().expect("probe log is not poisoned"),
            &[GroupProbeSite::GitExited],
            "supervision did not consume the post-exit group probe",
        );

        let live_sites = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&live_sites);
        let (mut process, workers) = supervised_process(move |site, _| {
            observed
                .lock()
                .expect("probe log is not poisoned")
                .push(site);
            match site {
                GroupProbeSite::GitExited => GroupPresence::Present,
                GroupProbeSite::TermSentBeforeKill | GroupProbeSite::KillSent => {
                    GroupPresence::Absent
                }
            }
        });

        let error = supervise_git(
            &mut process,
            workers,
            &["--version"],
            LOCAL_COMMAND_LIMIT,
            None,
        )
        .expect_err("a positively observed post-exit group is a survivor");
        assert_eq!(error.reason(), "git::descendant-survived");
        assert!(
            live_sites
                .lock()
                .expect("probe log is not poisoned")
                .contains(&GroupProbeSite::GitExited),
            "the survivor path did not consume the post-exit group probe",
        );
    }

    #[test]
    fn an_unreadable_pre_kill_probe_rechecks_an_already_absent_group() {
        let mut answers = [GroupPresence::Indeterminate, GroupPresence::Absent].into_iter();

        assert!(
            group_is_settled_against(GroupProbeSite::TermSentBeforeKill, || {
                Ok(answers.next().expect("the bounded probe has an answer"))
            })
            .expect("fixture observations are readable"),
            "the first unreadable answer skipped the re-probe and consumed the grace",
        );
        assert_eq!(
            answers.next(),
            None,
            "the pre-KILL decision exceeded its one-re-probe bound",
        );
    }

    #[test]
    fn an_unreadable_pre_kill_probe_never_settles_without_observed_absence() {
        for second in [GroupPresence::Present, GroupPresence::Indeterminate] {
            let mut answers = [GroupPresence::Indeterminate, second].into_iter();
            assert!(
                !group_is_settled_against(GroupProbeSite::TermSentBeforeKill, || Ok(answers
                    .next()
                    .expect("the bounded probe has an answer")),)
                .expect("fixture observations are readable"),
                "an unreadable group skipped KILL without observed absence",
            );
            assert_eq!(
                answers.next(),
                None,
                "the pre-KILL decision exceeded its one-re-probe bound",
            );
        }
    }

    #[test]
    fn terminate_escalates_on_an_unreadable_pre_kill_probe_and_forgives_it_after_kill() {
        let sites = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&sites);
        let mut process = spawn_process("/bin/sleep", &["30"], false, move |site, _| {
            observed
                .lock()
                .expect("probe log is not poisoned")
                .push(site);
            GroupPresence::Indeterminate
        });

        process
            .terminate()
            .expect("post-KILL uncertainty is not an observed survivor");

        let sites = sites.lock().expect("probe log is not poisoned");
        assert_eq!(
            sites.first(),
            Some(&GroupProbeSite::TermSentBeforeKill),
            "the pre-KILL settle did not consume its group probe",
        );
        assert_eq!(
            sites.get(1),
            Some(&GroupProbeSite::TermSentBeforeKill),
            "the unreadable pre-KILL probe was not re-read immediately",
        );
        assert_eq!(
            sites.last(),
            Some(&GroupProbeSite::KillSent),
            "the post-KILL settle did not consume its group probe",
        );
        assert!(
            !sites.contains(&GroupProbeSite::GitExited),
            "termination consumed the supervisor's post-exit probe site",
        );
    }
}
