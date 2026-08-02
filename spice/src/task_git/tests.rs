#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Output;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "spice-task-git-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create task Git scratch");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0)
                .expect("remove task Git scratch");
        }
    }

    struct Fixture {
        _scratch: Scratch,
        agent: PathBuf,
        peer: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            let remote = scratch.path().join("remote.git");
            git(
                scratch.path(),
                &["init", "--bare", "-b", "main", path(&remote)],
            );
            let agent = scratch.path().join("agent");
            git(
                scratch.path(),
                &["init", "-b", "main", path(&agent)],
            );
            identity(&agent);
            fs::write(agent.join("README.md"), "base\n")
                .expect("write base");
            git(&agent, &["add", "README.md"]);
            git(&agent, &["commit", "-m", "base"]);
            git(&agent, &["remote", "add", "origin", path(&remote)]);
            git(&agent, &["push", "-u", "origin", "main"]);
            git(&agent, &["remote", "set-head", "origin", "--auto"]);
            let peer = scratch.path().join("peer");
            git(
                scratch.path(),
                &["clone", path(&remote), path(&peer)],
            );
            identity(&peer);
            Self {
                _scratch: scratch,
                agent,
                peer,
            }
        }

        fn repository(&self) -> Repository {
            Repository::discover(&self.agent)
                .expect("discover agent repository")
        }

        fn agent_commit(&self, name: &str, content: &str) -> String {
            fs::write(self.agent.join(name), content)
                .expect("write agent path");
            git(&self.agent, &["add", name]);
            git(&self.agent, &["commit", "-m", "agent work"]);
            stdout(&self.agent, &["rev-parse", "HEAD"])
        }

        fn peer_commit(&self, name: &str, content: &str) -> String {
            fs::write(self.peer.join(name), content)
                .expect("write peer path");
            git(&self.peer, &["add", name]);
            git(&self.peer, &["commit", "-m", "peer work"]);
            git(&self.peer, &["push", "origin", "main"]);
            stdout(&self.peer, &["rev-parse", "HEAD"])
        }
    }

    fn metadata() -> Metadata<'static> {
        Metadata {
            label: "GIT-1kBoundary",
            title: "Publish lane work",
            project: "rim.task.git",
            phase: "todo",
            actor: "lane-fixture",
        }
    }

    #[test]
    fn publication_builds_baseline_first_merge_and_pushes_it() {
        let fixture = Fixture::new("publish");
        let agent = fixture.agent_commit("agent.txt", "agent\n");
        let upstream = fixture.peer_commit("peer.txt", "peer\n");

        let result = publish(&fixture.repository(), metadata())
            .expect("publish task work");

        assert_eq!(result.head, agent);
        assert_eq!(
            result.upstream_head.as_deref(),
            Some(upstream.as_str())
        );
        assert_eq!(
            parents(&fixture.agent, &result.integrated),
            vec![upstream, agent]
        );
        assert_eq!(
            stdout(
                &fixture.agent,
                &["ls-remote", "origin", "refs/heads/main"]
            )
            .split_whitespace()
            .next(),
            Some(result.integrated.as_str())
        );
        assert_eq!(
            stdout(&fixture.agent, &["status", "--porcelain"]),
            ""
        );
        assert_eq!(
            stdout(&fixture.agent, &["show", "HEAD:peer.txt"]),
            "peer"
        );
    }

    #[test]
    fn retry_after_published_merge_recovers_original_lane_head() {
        let fixture = Fixture::new("published-retry");
        let agent = fixture.agent_commit("agent.txt", "agent\n");
        let upstream = fixture.peer_commit("peer.txt", "peer\n");
        let first = publish(&fixture.repository(), metadata())
            .expect("first publication");

        let retried = publish(&fixture.repository(), metadata())
            .expect("retry published task transition");

        assert_eq!(retried.head, agent);
        assert_eq!(retried.integrated, first.integrated);
        assert_eq!(
            parents(&fixture.agent, &retried.integrated),
            vec![upstream, agent],
        );
    }

    #[test]
    fn capture_preserves_loose_head_and_claim_preparation_refuses_it()
    {
        let fixture = Fixture::new("capture");
        let loose = fixture.agent_commit("loose.txt", "loose\n");

        let refusal = prepare_claim(&fixture.repository())
            .expect_err("loose claim refused");
        let captured = capture(&fixture.repository())
            .expect("capture loose commit");

        assert_eq!(refusal.reason(), "git::claim-loose-commits");
        assert_eq!(captured.boundary.head, loose);
        assert_eq!(captured.boundary.integrated, loose);
        assert_eq!(captured.boundary.local_commits, 1);
        assert_eq!(captured.subject, "agent work");
        assert_eq!(
            stdout(&fixture.agent, &["rev-parse", "HEAD"]),
            loose
        );
    }

    #[test]
    fn conflict_leaves_complete_recoverable_merge_state() {
        let fixture = Fixture::new("conflict");
        fs::write(fixture.agent.join("README.md"), "agent\n")
            .expect("write agent conflict");
        git(&fixture.agent, &["add", "README.md"]);
        git(&fixture.agent, &["commit", "-m", "agent conflict"]);
        let agent = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
        fs::write(fixture.peer.join("README.md"), "peer\n")
            .expect("write peer conflict");
        git(&fixture.peer, &["add", "README.md"]);
        git(&fixture.peer, &["commit", "-m", "peer conflict"]);
        git(&fixture.peer, &["push", "origin", "main"]);
        let upstream = stdout(&fixture.peer, &["rev-parse", "HEAD"]);

        let error = publish(&fixture.repository(), metadata())
            .expect_err("conflict refuses");

        assert_eq!(error.reason(), "git::recoverable-conflict");
        assert_eq!(
            stdout(&fixture.agent, &["rev-parse", "HEAD"]),
            agent
        );
        assert_eq!(
            stdout(
                &fixture.agent,
                &["rev-parse", "--verify", "MERGE_HEAD"]
            ),
            upstream
        );
        assert!(
            stdout(&fixture.agent, &["status", "--short"])
                .starts_with("UU README.md")
        );
        assert!(
            fixture
                .agent
                .join("README.md")
                .read_to_string()
                .contains("<<<<<<<")
        );
        assert!(
            !transaction_path(&fixture.agent)
                .expect("transaction path")
                .exists()
        );
    }

    #[test]
    fn resolved_conflict_retry_preserves_the_resolution_commit() {
        let fixture = Fixture::new("conflict-retry");
        fs::write(fixture.agent.join("README.md"), "agent\n")
            .expect("write agent conflict");
        git(&fixture.agent, &["add", "README.md"]);
        git(&fixture.agent, &["commit", "-m", "agent conflict"]);
        let agent = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
        fs::write(fixture.peer.join("README.md"), "peer\n")
            .expect("write peer conflict");
        git(&fixture.peer, &["add", "README.md"]);
        git(&fixture.peer, &["commit", "-m", "peer conflict"]);
        git(&fixture.peer, &["push", "origin", "main"]);
        let upstream = stdout(&fixture.peer, &["rev-parse", "HEAD"]);
        publish(&fixture.repository(), metadata())
            .expect_err("conflict refuses");
        fs::write(fixture.agent.join("README.md"), "resolved\n")
            .expect("write resolution");
        git(&fixture.agent, &["add", "README.md"]);
        git(&fixture.agent, &["commit", "--no-edit"]);
        let resolved = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
        let marker = transaction_path(&fixture.agent)
            .expect("transaction path");
        write_pending(
            &marker,
            &Pending::Conflict {
                agent,
                upstream: upstream.clone(),
                previous: BTreeMap::new(),
            },
        )
        .expect("retain post-conflict crash marker");
        recover_pending(&fixture.agent)
            .expect("recognize committed resolution");

        let retried = publish(&fixture.repository(), metadata())
            .expect("publish resolved conflict");

        assert_eq!(retried.head, resolved);
        assert_eq!(
            parents(&fixture.agent, &retried.integrated),
            vec![upstream, resolved],
        );
        assert_eq!(
            stdout(&fixture.agent, &["show", "HEAD:README.md"]),
            "resolved",
        );
    }

    #[test]
    fn reference_hook_refusal_restores_the_pre_merge_checkout() {
        let fixture = Fixture::new("hook-refusal");
        let agent = fixture.agent_commit("agent.txt", "agent\n");
        let upstream = fixture.peer_commit("peer.txt", "peer\n");
        git(&fixture.agent, &["fetch", "origin"]);
        let hook = git_state_path(
            &fixture.agent,
            "hooks/reference-transaction",
        )
        .expect("reference hook path");
        fs::write(
            &hook,
            concat!(
                "#!/bin/sh\n",
                "case \"$1\" in\n",
                "prepared) exit 1 ;;\n",
                "*) exit 0 ;;\n",
                "esac\n",
            ),
        )
        .expect("write rejecting hook");
        let mut permissions =
            fs::metadata(&hook).expect("hook metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions)
            .expect("make hook executable");

        let message = compose_message(metadata(), &agent);
        let error = integrate(
            &fixture.agent,
            &agent,
            &upstream,
            &message,
            metadata().label,
        )
        .expect_err("hook refuses");

        assert_eq!(error.reason(), "git::ref-update-refused");
        assert_eq!(
            stdout(&fixture.agent, &["rev-parse", "HEAD"]),
            agent
        );
        assert_eq!(
            stdout(&fixture.agent, &["status", "--porcelain"]),
            ""
        );
        assert!(!fixture.agent.join("peer.txt").exists());
        assert!(
            !transaction_path(&fixture.agent)
                .expect("transaction path")
                .exists()
        );
    }

    #[test]
    fn reference_hook_pipe_descendant_is_bounded_and_reaped() {
        let fixture = Fixture::new("hook-descendant");
        let head = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
        let descendant_pid = fixture.agent.join(".git/hook-descendant-pid");
        let hook = git_state_path(
            &fixture.agent,
            "hooks/reference-transaction",
        )
        .expect("reference hook path");
        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "case \"$1\" in\n",
                "prepared)\n",
                "  /bin/sh -c 'trap \"\" HUP TERM; ",
                "printf \"%s\\n\" \"$$\" > {pid}; ",
                "exec /usr/bin/tail -f /dev/null' &\n",
                "  while [ ! -s {pid} ]; do sleep 0.01; done\n",
                ";;\n",
                "esac\n",
                "exit 0\n",
            ),
            pid = path(&descendant_pid),
        );
        fs::write(&hook, script).expect("write descendant hook");
        let mut permissions =
            fs::metadata(&hook).expect("hook metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).expect("make hook executable");

        let started = Instant::now();
        let error = run(
            &fixture.agent,
            &["update-ref", "refs/heads/hook-descendant", &head],
        )
        .expect_err("a hook descendant cannot outlive Git");

        assert_eq!(error.reason(), "git::descendant-survived");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "task Git exceeded its descendant cleanup bound: {:?}",
            started.elapsed(),
        );
        let pid = fs::read_to_string(&descendant_pid)
            .expect("read descendant pid")
            .trim()
            .parse::<i32>()
            .expect("descendant pid is numeric");
        let pid = rustix::process::Pid::from_raw(pid)
            .expect("descendant pid is positive");
        assert_eq!(
            rustix::process::test_kill_process(pid),
            Err(rustix::io::Errno::SRCH),
            "hook descendant remained alive after task Git returned",
        );
    }

    #[test]
    fn restart_repairs_both_sides_of_materialize_then_cas() {
        let fixture = Fixture::new("restart");
        let expected = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
        let target = fixture.agent_commit("target.txt", "target\n");
        git(
            &fixture.agent,
            &["update-ref", "refs/heads/main", &expected, &target],
        );
        let marker = transaction_path(&fixture.agent)
            .expect("transaction path");
        write_pending(
            &marker,
            &Pending::Move {
                expected: expected.clone(),
                target: target.clone(),
            },
        )
        .expect("write interrupted transaction");
        git(&fixture.agent, &["read-tree", "--reset", "-u", &target]);

        recover_pending(&fixture.agent)
            .expect("repair pre-CAS crash");

        assert_eq!(
            stdout(&fixture.agent, &["rev-parse", "HEAD"]),
            expected
        );
        assert!(!fixture.agent.join("target.txt").exists());
        write_pending(
            &marker,
            &Pending::Move {
                expected: expected.clone(),
                target: target.clone(),
            },
        )
        .expect("write post-CAS transaction");
        git(
            &fixture.agent,
            &["update-ref", "refs/heads/main", &target, &expected],
        );

        recover_pending(&fixture.agent)
            .expect("repair post-CAS crash");

        assert_eq!(
            stdout(&fixture.agent, &["rev-parse", "HEAD"]),
            target
        );
        assert_eq!(
            fs::read_to_string(fixture.agent.join("target.txt"))
                .expect("read target"),
            "target\n"
        );
        assert_eq!(
            stdout(&fixture.agent, &["status", "--porcelain"]),
            ""
        );
    }

    #[test]
    fn publication_race_retries_are_bounded_without_losing_commits() {
        let fixture = Fixture::new("race-bound");
        let agent = fixture.agent_commit("agent.txt", "agent\n");
        let counter = fixture.agent.join(".git/race-count");
        let hook = fixture.agent.join(".git/hooks/pre-push");
        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "count=0\n",
                "[ ! -f {counter} ] || ",
                "count=$(cat {counter})\n",
                "count=$((count + 1))\n",
                "printf '%s\\n' \"$count\" > {counter}\n",
                "git -C {peer} pull --ff-only origin main\n",
                "printf 'peer %s\\n' \"$count\" > ",
                "{peer}/peer-$count.txt\n",
                "git -C {peer} add peer-$count.txt\n",
                "git -C {peer} commit -m ",
                "\"peer race $count\"\n",
                "git -C {peer} push origin main\n",
            ),
            counter = path(&counter),
            peer = path(&fixture.peer),
        );
        fs::write(&hook, script).expect("write racing pre-push hook");
        let mut permissions =
            fs::metadata(&hook).expect("hook metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions)
            .expect("make hook executable");

        let error = publish(&fixture.repository(), metadata())
            .expect_err("race bound refuses");

        assert_eq!(
            error.reason(),
            "git::publish-race-exhausted",
            "{}",
            error.detail()
        );
        assert_eq!(
            fs::read_to_string(&counter)
                .expect("read push count")
                .trim(),
            "6"
        );
        let current = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
        assert_eq!(
            stdout(
                &fixture.agent,
                &["show", &format!("{current}:agent.txt")]
            ),
            "agent"
        );
        assert!(
            is_ancestor(&fixture.agent, &agent, &current)
                .expect("agent ancestry")
        );
        assert_eq!(
            stdout(&fixture.agent, &["status", "--porcelain"]),
            ""
        );
    }

    trait ReadText {
        fn read_to_string(&self) -> String;
    }

    impl ReadText for PathBuf {
        fn read_to_string(&self) -> String {
            fs::read_to_string(self).expect("read text path")
        }
    }

    fn path(path: &Path) -> &str {
        path.to_str().expect("fixture path is UTF-8")
    }

    fn identity(root: &Path) {
        git(root, &["config", "user.name", "Spice Task Git"]);
        git(root, &["config", "user.email", "spice@example.test"]);
    }

    fn parents(root: &Path, commit: &str) -> Vec<String> {
        stdout(root, &["show", "-s", "--format=%P", commit])
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    fn stdout(root: &Path, args: &[&str]) -> String {
        let output = test_git(root, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn git(root: &Path, args: &[&str]) {
        let output = test_git(root, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn test_git(root: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_KEY_0")
            .env_remove("GIT_CONFIG_VALUE_0")
            .env_remove("GIT_CONFIG_KEY_1")
            .env_remove("GIT_CONFIG_VALUE_1")
            .output()
            .expect("launch fixture Git")
    }
}
