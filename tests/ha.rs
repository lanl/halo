// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use halo_lib::{
        commands::status::get_status,
        config::{self, Config},
        test_env::*,
    };

    /// Holds state related to a single HA test.
    struct HaEnvironment {
        env: TestEnvironment,
        ports: [u16; 2],
        test_id: String,
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
            env.write_out_config(config);
            Self {
                env,
                test_id: test_id.to_string(),
                ports,
            }
        }

        fn start_agent(&self, which_one: usize) -> ChildHandle {
            let agent = TestAgent {
                port: self.ports[which_one],
                id: Some(self.test_id.clone()),
            };

            self.env
                .start_remote_agents(vec![agent])
                .into_iter()
                .next()
                .unwrap()
        }

        fn start_manager(&self) -> ChildHandle {
            self.env.start_manager()
        }

        fn socket_path(&self) -> String {
            self.env.socket_path()
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

    #[test]
    fn startup_agents_running_resources_stopped() {
        let env = HaEnvironment::new("startup_agents_running_resources_stopped");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager();

        // Sleep for a second to give the manager enough time to start resources...
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Then run the status command to be sure the resources started
        let cluster_status = get_status(&env.socket_path()).unwrap();
        eprintln!("{cluster_status:?}");

        for res in cluster_status.resources {
            assert_eq!(res.status, "Running");
        }
    }
}
