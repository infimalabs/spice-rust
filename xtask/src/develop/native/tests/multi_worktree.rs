use super::*;

impl Fixture {
    /// Create a linked worktree that shares this fixture's installed appliance.
    fn linked_repository(&self, name: &str) -> Repository {
        let admin = self.0.join(".git/worktrees").join(name);
        let worktree = self.0.join("linked").join(name);
        fs::create_dir_all(&admin).expect("linked worktree administration is created");
        fs::create_dir_all(&worktree).expect("linked worktree is created");
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .expect("linked worktree git pointer writes");
        fs::write(admin.join("commondir"), "../..\n").expect("linked common pointer writes");
        fs::write(admin.join("HEAD"), "ref: refs/heads/main\n")
            .expect("linked worktree HEAD writes");
        seed_app_stage(&worktree);
        Repository::discover(worktree).expect("linked repository resolves")
    }
}

/// Publications from linked worktrees must not displace the live generation's
/// command authority with an intermediate installed image.
#[test]
fn publications_retain_one_shared_resident_authority() {
    let fixture = Fixture::new("linked-worktree-resident-authority");
    let original = fixture.repository();
    let lane_b = fixture.linked_repository("lane-b");
    let lane_c = fixture.linked_repository("lane-c");
    let lane_z = fixture.linked_repository("lane-z");
    let state = original.admit_state().expect("fixture state is admitted");
    let discovery_path = state.serve().to_path_buf();
    let stopped = fixture.0.join("original-stopped");
    let started = fixture.0.join("lane-z-started");
    let saved_discovery = fixture.0.join("saved-discovery");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bootstrap listener binds");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("bootstrap address reads")
    );
    let predecessor = fixture.binary(
        "spice-original",
        format!(
            "#!/bin/sh\nif [ \"$1\" = down ]; then rm -f '{}'; : > '{}'; exit 0; fi\nexit 9\n",
            discovery_path.display(),
            stopped.display(),
        )
        .as_bytes(),
    );
    refresh_install(&original, &predecessor).expect("original executable installs");
    let predecessor_binary =
        executable_digest(&install::installed_binary(&original)).expect("original hashes");
    let discovery = fixture.publish(&endpoint);
    fs::copy(&discovery_path, &saved_discovery).expect("discovery is retained for successor");
    let readiness = wire::Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::AppOpen {
            endpoint: endpoint.clone(),
            epoch: discovery.epoch,
        },
    });
    let readiness = String::from_utf8(wire::encode(&readiness).expect("readiness encodes"))
        .expect("readiness is UTF-8");
    let successor_b = fixture.binary("spice-lane-b", b"lane b executable\n");
    let successor_c = fixture.binary("spice-lane-c", b"lane c executable\n");
    let successor_z = fixture.binary(
        "spice-lane-z",
        format!(
            "#!/bin/sh\ncp '{}' '{}'\n: > '{}'\nprintf '%s\\n' '{}'\n",
            saved_discovery.display(),
            discovery_path.display(),
            started.display(),
            readiness,
        )
        .as_bytes(),
    );
    let successor_z_binary = executable_digest(&successor_z).expect("lane z successor hashes");
    let server = serve_bootstraps(
        listener,
        discovery,
        vec![
            predecessor_binary.clone(),
            predecessor_binary.clone(),
            predecessor_binary.clone(),
            successor_z_binary.clone(),
        ],
    );
    let no_resident = RefreshRequest {
        restart: false,
        ..RefreshRequest::default()
    };
    let mut output = Vec::new();

    install_built(&lane_b, successor_b.clone(), &no_resident, &mut output)
        .expect("lane b publishes without restarting");
    install_built(&lane_c, successor_c, &no_resident, &mut output)
        .expect("lane c publishes without restarting");

    let common = workspace(&lane_z).expect("common refresh workspace exists");
    assert_eq!(
        executable_digest(&common.join(RESIDENT_ROLLBACK)).expect("resident authority hashes"),
        predecessor_binary,
        "later publications displaced the still-serving executable",
    );
    assert_eq!(
        executable_digest(&common.join(ROLLBACK)).expect("transaction rollback hashes"),
        executable_digest(&successor_b).expect("lane b executable hashes"),
        "the immediate install rollback did not follow the installed generation",
    );

    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };
    let report = install_built(&lane_z, successor_z, &request, &mut output)
        .expect("lane z replaces the original resident");
    let (_, running) = running_resident_version(&lane_z).expect("lane z bootstrap answers");
    server.join().expect("bootstrap server joins");

    assert!(stopped.exists(), "original resident did not stop");
    assert!(started.exists(), "lane z successor did not launch");
    assert_eq!(report.endpoint.as_deref(), Some(endpoint.as_str()));
    assert_eq!(running.binary, successor_z_binary);
    assert!(
        !common.join(RESIDENT_ROLLBACK).exists(),
        "retired resident authority survived successor readiness",
    );
}
