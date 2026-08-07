// SPDX-License-Identifier: MIT
// Copyright 2026. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use halo_lib::test_env::ha::*;

    /// Create a TestEnvironment for a test.
    ///
    /// The path to the remote binary needs to be determined here and passed into the
    /// TestEnvironment constructor because the environment variable is only defined when compiling
    /// tests.
    fn test_env_helper(test_id: &str) -> HaEnvironment {
        HaEnvironment::new_shared(
            test_id.to_string(),
            env!("CARGO_BIN_EXE_halo_remote"),
            env!("CARGO_BIN_EXE_halo_manager"),
        )
    }

    /// Startup, both agents running, all resources stopped.
    /// Agents should start resources on their home nodes.
    #[test]
    fn startup1() {
        let env = test_env_helper("startup1");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let status = env.get_status();
        for resource in status.resources {
            let (st, _) = resource.single_host_status();
            match resource.kind.as_str() {
                "heartbeat/ip" => assert_eq!(st, "Running"),
                _ => {
                    for st in resource.status.values() {
                        assert_eq!(st.status, "Running");
                    }
                }
            }
        }
    }

    /// All resources running on one host, after failback, exclusive resources should be running on
    /// their home host and shared resources should be running everywhere.
    #[test]
    fn failback1() {
        let env = test_env_helper("failback1");

        env.start_resource("ip_addr_0", 0);
        env.start_resource("ip_addr_1", 0);

        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));

        let status = env.get_status();
        for resource in status.resources {
            let st0 = resource.status.get(&env.agent_id(0)).unwrap();
            assert_eq!(st0.status, "Running");

            let st1 = &resource.status.get(&env.agent_id(1)).unwrap().status;
            assert!((st1 == "Stopped") | (st1 == "Unknown"));
        }

        env.failback(1).unwrap();

        std::thread::sleep(std::time::Duration::from_secs(1));

        let status = env.get_status();
        for resource in status.resources {
            let (st, _) = resource.single_host_status();
            match resource.kind.as_str() {
                "heartbeat/ip" => assert_eq!(st, "Running"),
                _ => {
                    for st in resource.status.values() {
                        assert_eq!(st.status, "Running");
                    }
                }
            }
        }
    }
}
