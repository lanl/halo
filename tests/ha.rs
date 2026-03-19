// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use halo_lib::test_env::*;

    /// Create a TestEnvironment for a test.
    ///
    /// The path to the remote binary needs to be determined here and passed into the
    /// TestEnvironment constructor because the environment variable is only defined when compiling
    /// tests.
    fn test_env_helper(test_id: &str) -> HaEnvironment {
        HaEnvironment::new(
            test_id.to_string(),
            env!("CARGO_BIN_EXE_halo_remote"),
            env!("CARGO_BIN_EXE_halo_manager"),
        )
    }

    /// Make sure that the test environment correctly detects a test that results in a
    /// "double-started" resource. This should panic the test, normally causing it to fail, but in
    /// this test we want to make sure the panic happens.
    #[test]
    #[should_panic]
    fn double_started_resource() {
        let env = test_env_helper("double_started_resource");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        // Sleep for a second to give the manager enough time to start resources...
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Manually start the zpool on the failover node, even though it's already started on the
        // home node:
        env.start_resource("zpool_0", 1);

        // Drop implementation of HaEnvironment is expected to panic here...
    }

    /// Startup, both agents running, all resources stopped.
    /// Agents should start resources on their home nodes.
    #[test]
    fn startup1() {
        let env = test_env_helper("startup1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        // Sleep for a second to give the manager enough time to start resources...
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Then run the status command to be sure the resources started
        let cluster_status = env.get_status();

        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }
    }

    /// Startup, one agent stopped, all resources stopped.
    /// All resources should enter "error" status because the system cannot tell if they are
    /// running on the "down" node so it isn't safe to start them.
    #[test]
    fn startup2() {
        let env = test_env_helper("startup2");
        let _a = env.start_agent(0);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();

        for res in cluster_status.resources {
            assert_eq!(res.status, "Error");
        }
    }

    /// Startup, one agent stopped, one running. Up node resources are running locally, down node
    /// resources are not.
    /// Manager should report up resources as running, down resources as Error since it cannot tell
    /// if they are started anywhere.
    #[test]
    fn startup3() {
        let env = test_env_helper("startup3");

        // Just starting the resource group root is enough to get HALO to treat the entire resource
        // group as located on a particular node.
        env.start_resource("zpool_0", 0);

        let _a = env.start_agent(0);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();

        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Error");
            }
        }
    }

    /// Startup, one agent stopped, one running, all resources started on running node.
    /// Manager should report all resources as running, with the correct ones as failed over.
    #[test]
    fn startup4() {
        let env = test_env_helper("startup4");

        env.start_resource("zpool_0", 0);
        env.start_resource("zpool_1", 0);

        let _a = env.start_agent(0);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();

        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Running (Failed Over)");
            }
        }
    }

    /// Startup, both agents down - there is nothing the manager can do, report resources in error
    /// status.
    ///
    /// Then, once both agents start, the resources can be started.
    #[test]
    fn startup5() {
        let env = test_env_helper("startup5");

        // Not starting agents...
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();

        for res in cluster_status.resources {
            assert_eq!(res.status, "Error");
        }

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }

        // Make sure these drop first, just to avoid noise in the error output...
        drop(_a);
        drop(_b);
    }

    /// Startup, one agent down, all resources running on down node. Resources should be in error
    /// status.
    /// Start the down agent, all resources should go to running, with the correct ones failed
    /// over.
    #[test]
    fn startup6() {
        let env = test_env_helper("startup6");

        env.start_resource("zpool_0", 0);
        env.start_resource("zpool_1", 0);

        let _a = env.start_agent(1); // Start only the agent where no resources are running.

        let _m = env.start_manager(true);

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Error");
        }

        let _b = env.start_agent(0); // Now start the agent where the resources are running.

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Running (Failed Over)");
            }
        }

        drop(_b);
    }

    /// Failover - All resources start out on home.
    #[test]
    fn failover1() {
        let env = test_env_helper("failover1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        // Stop the remote agent to trigger failover:
        drop(_b);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Running (Failed Over)");
            }
        }
    }

    /// Failover - both resource groups running on same node, both get failed over.
    #[test]
    fn failover2() {
        let env = test_env_helper("failover2");

        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 0);
        env.start_resource("mdt_1", 0);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Running (Failed Over)");
            }
        }

        // Stop the remote agent to trigger failover:
        drop(_a);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    /// Failover - resource groups start failed over, failback command is used to bring them home.
    #[test]
    fn failover3() {
        let env = test_env_helper("failover3");

        env.start_resource("zpool_0", 1);
        env.start_resource("mdt_0", 1);
        env.start_resource("zpool_1", 0);
        env.start_resource("mdt_1", 0);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Running (Failed Over)");
        }

        env.failback(0).unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Running (Failed Over)");
            }
        }

        env.failback(1).unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }
    }

    /// Failover - failback command should have no effect when resources are not failed over.
    #[test]
    fn failover4() {
        let env = test_env_helper("failover4");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        // Wait for manager to start the resources
        std::thread::sleep(std::time::Duration::from_secs(1));

        env.failback(0).unwrap();
        env.failback(1).unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }
    }

    /// Failover - after manager triggers failover, if agent starts, failback should bring resources
    /// back.
    #[test]
    fn failover5() {
        let env = test_env_helper("failover5");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        // Stop the remote agent to trigger failover:
        drop(_b);

        // Wait for failover to occur
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Restart remote agent...
        let _b = env.start_agent(1);

        // Wait for manager to reconnect
        std::thread::sleep(std::time::Duration::from_secs(2));

        env.failback(1).unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }
    }

    /// Observe mode - test that a resource stays stopped
    #[test]
    fn observe1() {
        let env = test_env_helper("observe1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(false);

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Stopped");
        }
    }

    /// Observe mode - test that a resource already started shows up as started, and failed over if
    /// appropriate.
    #[test]
    fn observe2() {
        let env = test_env_helper("observe2");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(false);

        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 0);
        env.start_resource("mdt_1", 0);

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Running (Failed Over)");
            }
        }
    }

    fn observe_start_and_stop(env: HaEnvironment, manual_fail_over: bool) {
        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 1);
        env.start_resource("mdt_1", 1);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(false);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }

        env.stop_resource("mdt_1", 1);
        env.stop_resource("zpool_1", 1);

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Stopped");
            }
        }

        if manual_fail_over {
            env.start_resource("zpool_1", 0);
            env.start_resource("mdt_1", 0);
        } else {
            env.start_resource("zpool_1", 1);
            env.start_resource("mdt_1", 1);
        }

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running");
            } else if manual_fail_over {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    /// Observe mode - test that a resource started, stopped, and then started on the same node,
    /// shows the correct statuses
    #[test]
    fn observe3() {
        let env = test_env_helper("observe3");

        observe_start_and_stop(env, false);
    }

    /// Observe mode - test that a resource started, stopped, and then started on the partner,
    /// shows the correct statuses
    #[test]
    fn observe4() {
        let env = test_env_helper("observe4");

        observe_start_and_stop(env, true);
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then it is started on the same node - status should correctly report that it is started.
    #[test]
    fn unmanage1() {
        let env = test_env_helper("unmanage1");
        unmanage_then_stop_and_start(env, false);
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then it is started on the failover node - status should correctly report that it is
    /// started, but failed over.
    #[test]
    fn unmanage2() {
        let env = test_env_helper("unmanage2");
        unmanage_then_stop_and_start(env, true);
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then it is re-managed - manager should start it on the home node.
    #[test]
    fn unmanage3() {
        let env = test_env_helper("unmanage3");
        unmanage_then_stop_and_remanage(env, None);
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then the root resources is started on the failover node, and it is re-managed. Manager
    /// should start the child resource on the failover node.
    #[test]
    fn unmanage4() {
        let env = test_env_helper("unmanage4");
        unmanage_then_stop_and_remanage(env, Some(true));
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then the root resources is started on the home node, and it is re-managed. Manager
    /// should start the child resource on the home node.
    #[test]
    fn unmanage5() {
        let env = test_env_helper("unmanage5");
        unmanage_then_stop_and_remanage(env, Some(false));
    }

    fn unmanage_then_stop_and_start(env: HaEnvironment, manual_fail_over: bool) {
        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 1);
        env.start_resource("mdt_1", 1);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        unmanage_then_stop(&env);

        // First start just the root resource...
        if manual_fail_over {
            env.start_resource("zpool_0", 1);
        } else {
            env.start_resource("zpool_0", 0);
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id == "mdt_0" {
                assert_eq!(res.status, "Stopped");
            } else if res.id == "zpool_0" && manual_fail_over {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }

        // ...then start the child resource.
        if manual_fail_over {
            env.start_resource("mdt_0", 1);
        } else {
            env.start_resource("mdt_0", 0);
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if manual_fail_over && res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    #[test]
    fn admin_fence1() {
        let env = test_env_helper("admin_fence1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        // Sleep for a second to give the manager enough time to start resources...
        std::thread::sleep(std::time::Duration::from_secs(1));

        env.fence(0, false).unwrap();

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
        for host in cluster_status.hosts {
            if host.id.ends_with("0") {
                assert!(!host.connected)
            } else {
                assert!(host.connected)
            }
        }
    }

    /// Fence node while holding both its own and partner resources
    #[test]
    fn admin_fence2() {
        let env = test_env_helper("admin_fence2");

        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 0);
        env.start_resource("mdt_1", 0);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        // Sleep for a second to give the manager enough time to start resources...
        std::thread::sleep(std::time::Duration::from_secs(1));

        env.fence(0, false).unwrap();

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
        for host in cluster_status.hosts {
            if host.id.ends_with("0") {
                assert!(!host.connected)
            } else {
                assert!(host.connected)
            }
        }
    }

    /// start_failed_over:
    /// - None: do not start resources manually, let manager start
    /// - Some(true): start resources manually on FAILOVER node
    /// - Some(false): start resources manually on HOME node
    fn unmanage_then_stop_and_remanage(env: HaEnvironment, start_failed_over: Option<bool>) {
        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 1);
        env.start_resource("mdt_1", 1);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        unmanage_then_stop(&env);

        match start_failed_over {
            Some(true) => env.start_resource("zpool_0", 1),
            Some(false) => env.start_resource("zpool_0", 0),
            None => {}
        };

        env.manage_resource("zpool_0");

        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if start_failed_over == Some(true) && res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    fn unmanage_then_stop(env: &HaEnvironment) {
        std::thread::sleep(std::time::Duration::from_secs(2));

        env.unmanage_resource("zpool_0");

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert!(!res.managed);
                assert_eq!(res.status, "Running");
            } else {
                assert_eq!(res.status, "Running");
            }
        }

        env.stop_resource("mdt_0", 0);
        env.stop_resource("zpool_0", 0);
        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Stopped");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    /// When a host is deactivated, it should gracefully stop the resources and bring them up on
    /// the partner host.
    ///
    /// After reactivating the host, it should be possible to fail back the resources onto it.
    #[test]
    fn deactivate1() {
        let env = test_env_helper("deactivate1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        deactivate_one_host(&env);

        env.activate_host(0);
        std::thread::sleep(std::time::Duration::from_secs(1));

        env.failback(0).unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for host in cluster_status.hosts {
            assert!(host.active)
        }
        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }
    }

    /// Deactivate a host.
    /// Then, stop the partner host. Resources should enter Unknown status rather than run on the
    /// deactivated host.
    ///
    /// Then, start up the partner host. Resources should be restarted on it.
    #[test]
    fn deactivate2() {
        let env = test_env_helper("deactivate2");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        deactivate_one_host(&env);

        drop(_b);

        std::thread::sleep(std::time::Duration::from_secs(3));
        let cluster_status = env.get_status();
        for host in cluster_status.hosts {
            if host.id.contains("0") {
                assert!(!host.active);
                assert!(host.connected);
            } else {
                assert!(host.active);
                assert!(!host.connected);
            }
        }

        for res in cluster_status.resources {
            assert_eq!(res.status, "Unknown");
        }

        let _b = env.start_agent(1);

        std::thread::sleep(std::time::Duration::from_secs(2));
        let cluster_status = env.get_status();
        for host in cluster_status.hosts {
            if host.id.contains("0") {
                assert!(!host.active);
                assert!(host.connected);
            } else {
                assert!(host.active);
                assert!(host.connected);
            }
        }

        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    /// If a host is deactivated, a failback onto that host should not succeed.
    #[test]
    fn deactivate3() {
        let env = test_env_helper("deactivate3");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        deactivate_one_host(&env);

        assert!(env.failback(0).is_err());
    }

    /// Startup - a host is deactivated, but still running resources. The manager should notice
    /// that the resources are running on the deactivated host, and move them over to the activated
    /// host.
    #[test]
    fn deactivate4() {
        let env = test_env_helper("deactivate4");
        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 1);
        env.start_resource("mdt_1", 1);
        let _m = env.start_manager(true);
        env.deactivate_host(0);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);

        std::thread::sleep(std::time::Duration::from_secs(2));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    fn deactivate_one_host(env: &HaEnvironment) {
        std::thread::sleep(std::time::Duration::from_secs(1));
        env.deactivate_host(0);
        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
        for host in cluster_status.hosts {
            if host.id.contains("0") {
                assert!(!host.active)
            } else {
                assert!(host.active)
            }
        }
    }

    // Fencing cannot happen twice without reset happening
    #[test]
    fn fence_reset1() {
        let env = test_env_helper("fence_reset1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(_b);
        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();
        for host in cluster_status.hosts {
            if host.id.ends_with("1") {
                assert!(host.fenced)
            } else {
                assert!(!host.fenced)
            }
        }

        let _b = env.start_agent(1);
        env.failback(1).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));

        drop(_b);
        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("1") {
                assert_eq!(res.status, "Unknown");
            } else {
                assert_eq!(res.status, "Running");
            }
        }

        env.reset_host(1);
        let cluster_status = env.get_status();
        for host in cluster_status.hosts {
            assert!(!host.fenced)
        }

        let _b = env.start_agent(1);
        env.failback(1).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(_b);
        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("1") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
        for host in cluster_status.hosts {
            if host.id.ends_with("1") {
                assert!(host.fenced)
            } else {
                assert!(!host.fenced)
            }
        }
    }

    // Fencing - before reset, fence command results in error
    #[test]
    fn fence_reset2() {
        let env = test_env_helper("fence_reset2");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(_b);
        std::thread::sleep(std::time::Duration::from_secs(1));

        let _b = env.start_agent(1);
        env.failback(1).unwrap();

        assert!(env.fence(1, false).is_err());
    }
}
