// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use halo_lib::{
        commands::{self, status::get_status},
        config::{self, Config},
        test_env::*,
    };

    /// Holds state related to a single HA test.
    struct HaEnvironment {
        env: TestEnvironment,
        ports: [u16; 2],
        test_id: String,
        config: Config,
    }

    impl HaEnvironment {
        fn new(test_id: &str) -> Self {
            let ports = get_ports();
            let env = TestEnvironment::new(
                test_id.to_string(),
                env!("CARGO_BIN_EXE_halo_remote"),
                env!("CARGO_BIN_EXE_halo_manager"),
            );
            let config = ha_config(ports, test_id.to_string());
            env.write_out_config(&config);
            Self {
                env,
                test_id: test_id.to_string(),
                ports,
                config,
            }
        }

        fn start_agent(&self, which_one: usize) -> ChildHandle {
            let agent = TestAgent {
                port: self.ports[which_one],
                id: Some(format!("{}_{}", self.test_id, which_one)),
            };

            self.env
                .start_remote_agents(vec![agent])
                .into_iter()
                .next()
                .unwrap()
        }

        fn start_manager(&self, manage_resources: bool) -> ManagerHandle {
            self.env.start_manager(manage_resources)
        }

        fn socket_path(&self) -> String {
            self.env.socket_path()
        }

        fn start_resource(&self, resource_id: &str, which_agent: usize) {
            for host in &self.config.hosts {
                if let Some(resource) = host.resources.get(resource_id) {
                    self.env.start_resource(resource, which_agent);
                    return;
                }
            }

            panic!("Unable to find resource with id {resource_id}");
        }

        fn stop_resource(&self, resource_id: &str, which_agent: usize) {
            for host in &self.config.hosts {
                if let Some(resource) = host.resources.get(resource_id) {
                    self.env.stop_resource(resource, which_agent);
                    return;
                }
            }

            panic!("Unable to find resource with id {resource_id}");
        }

        fn manage_resource(&self, resource_id: &str) {
            commands::manage::send_command(&Some(self.socket_path()), resource_id, true).unwrap();
        }

        fn unmanage_resource(&self, resource_id: &str) {
            commands::manage::send_command(&Some(self.socket_path()), resource_id, false).unwrap();
        }
    }

    /// Get a pair of ports to use for a test.
    fn get_ports() -> [u16; 2] {
        static COUNTER: Mutex<u16> = Mutex::new(8100);
        let mut port = COUNTER.lock().unwrap();
        let val = *port;
        *port += 2;
        [val, val + 1]
    }

    /// Creates an HA-pair config for use in the ha tests.
    fn ha_config(ports: [u16; 2], test_id: String) -> Config {
        let mut config = Config {
            hosts: Vec::new(),
            failover_pairs: Some(vec![vec![
                format!("127.0.0.1:{}", ports[0]),
                format!("127.0.0.1:{}", ports[1]),
            ]]),
        };

        for i in 0..2 {
            let zpool_name = || -> String { format!("zpool_{i}") };
            let lustre_name = || -> String { format!("mdt_{i}") };

            let root_resource = config::Resource {
                kind: "heartbeat/ZFS".to_string(),
                parameters: HashMap::from([("pool".to_string(), zpool_name())]),
                requires: None,
            };

            let child_resource = config::Resource {
                kind: "lustre/Lustre".to_string(),
                parameters: HashMap::from([
                    ("mountpoint".to_string(), lustre_name()),
                    ("target".to_string(), lustre_name()),
                    ("kind".to_string(), "mdt".to_string()),
                ]),
                requires: Some(zpool_name()),
            };

            let host = config::Host {
                hostname: format!("127.0.0.1:{}", ports[i]),
                resources: HashMap::from([
                    (zpool_name(), root_resource),
                    (lustre_name(), child_resource),
                ]),
                fence_agent: Some("fence_test".to_string()),
                fence_parameters: Some(HashMap::from([
                    ("target".to_string(), format!("fence_mds_{i}")),
                    ("test_id".to_string(), test_id.clone()),
                ])),
            };

            config.hosts.push(host);
        }

        config
    }

    /// Startup, both agents running, all resources stopped.
    /// Agents should start resources on their home nodes.
    #[test]
    fn startup1() {
        let env = HaEnvironment::new("startup1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        // Sleep for a second to give the manager enough time to start resources...
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Then run the status command to be sure the resources started
        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");

        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }
    }

    /// Startup, one agent stopped, all resources stopped.
    /// All resources should enter "error" status because the system cannot tell if they are
    /// running on the "down" node so it isn't safe to start them.
    #[test]
    fn startup2() {
        let env = HaEnvironment::new("startup2");
        let _a = env.start_agent(0);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");

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
        let env = HaEnvironment::new("startup3");

        // Just starting the resource group root is enough to get HALO to treat the entire resource
        // group as located on a particular node.
        env.start_resource("zpool_0", 0);

        let _a = env.start_agent(0);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");

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
        let env = HaEnvironment::new("startup4");

        env.start_resource("zpool_0", 0);
        env.start_resource("zpool_1", 0);

        let _a = env.start_agent(0);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");

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
        let env = HaEnvironment::new("startup5");

        // Not starting agents...
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");

        for res in cluster_status.resources {
            assert_eq!(res.status, "Error");
        }

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");
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
        let env = HaEnvironment::new("startup6");

        env.start_resource("zpool_0", 0);
        env.start_resource("zpool_1", 0);

        let _a = env.start_agent(1); // Start only the agent where no resources are running.

        let _m = env.start_manager(true);

        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");
        for res in cluster_status.resources {
            assert_eq!(res.status, "Error");
        }

        let _b = env.start_agent(0); // Now start the agent where the resources are running.

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = get_status(&env.socket_path()).unwrap();
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
        let env = HaEnvironment::new("failover1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        // Stop the remote agent to trigger failover:
        drop(_b);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");
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
        let env = HaEnvironment::new("failover2");

        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 0);
        env.start_resource("mdt_1", 0);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = get_status(&env.socket_path()).unwrap();
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

        let cluster_status = get_status(&env.socket_path()).unwrap();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    /// Observe mode - test that a resource stays stopped
    #[test]
    fn observe1() {
        let env = HaEnvironment::new("observe1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(false);

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = get_status(&env.socket_path()).unwrap();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Stopped");
        }
    }

    /// Observe mode - test that a resource already started shows up as started, and failed over if
    /// appropriate.
    #[test]
    fn observe2() {
        let env = HaEnvironment::new("observe2");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(false);

        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 0);
        env.start_resource("mdt_1", 0);

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = get_status(&env.socket_path()).unwrap();
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

        let cluster_status = get_status(&env.socket_path()).unwrap();
        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }

        env.stop_resource("mdt_1", 1);
        env.stop_resource("zpool_1", 1);

        std::thread::sleep(std::time::Duration::from_secs(2));

        let cluster_status = get_status(&env.socket_path()).unwrap();
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

        let cluster_status = get_status(&env.socket_path()).unwrap();
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
        let env = HaEnvironment::new("observe3");

        observe_start_and_stop(env, false);
    }

    /// Observe mode - test that a resource started, stopped, and then started on the partner,
    /// shows the correct statuses
    #[test]
    fn observe4() {
        let env = HaEnvironment::new("observe4");

        observe_start_and_stop(env, true);
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then it is started on the same node - status should correctly report that it is started.
    #[test]
    fn unmanage1() {
        let env = HaEnvironment::new("unmanage1");
        unmanage_then_stop_and_start(env, false);
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then it is started on the failover node - status should correctly report that it is
    /// started, but failed over.
    #[test]
    fn unmanage2() {
        let env = HaEnvironment::new("unmanage2");
        unmanage_then_stop_and_start(env, true);
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then it is re-managed - manager should start it on the home node.
    #[test]
    fn unmanage3() {
        let env = HaEnvironment::new("unmanage3");
        unmanage_then_stop_and_remanage(env, false);
    }

    /// A resource is unmanaged, then manually stopped - status should correctly report that it is
    /// stopped.
    ///
    /// Then the root resources is started on the failover node, and it is re-managed. Manager
    /// should start the child resource on the failover node.
    #[test]
    fn unmanage4() {
        let env = HaEnvironment::new("unmanage4");
        unmanage_then_stop_and_remanage(env, true);
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
        let cluster_status = get_status(&env.socket_path()).unwrap();
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
        let cluster_status = get_status(&env.socket_path()).unwrap();
        for res in cluster_status.resources {
            if manual_fail_over && res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    fn unmanage_then_stop_and_remanage(env: HaEnvironment, manual_fail_over: bool) {
        env.start_resource("zpool_0", 0);
        env.start_resource("mdt_0", 0);
        env.start_resource("zpool_1", 1);
        env.start_resource("mdt_1", 1);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        unmanage_then_stop(&env);

        if manual_fail_over {
            env.start_resource("zpool_0", 1);
        }

        env.manage_resource("zpool_0");

        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = get_status(&env.socket_path()).unwrap();
        for res in cluster_status.resources {
            if manual_fail_over && res.id.contains("0") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }

    fn unmanage_then_stop(env: &HaEnvironment) {
        std::thread::sleep(std::time::Duration::from_secs(2));

        env.unmanage_resource("zpool_0");

        let cluster_status = get_status(&env.socket_path()).unwrap();
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
        let cluster_status = get_status(&env.socket_path()).unwrap();
        for res in cluster_status.resources {
            if res.id.contains("0") {
                assert_eq!(res.status, "Stopped");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }
}
