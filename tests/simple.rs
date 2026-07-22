// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use halo_lib::{host::FenceCommand, test_env::*};

    /// Create a TestEnvironment for a test.
    ///
    /// The path to the remote binary needs to be determined here and passed into the
    /// TestEnvironment constructor because the environment variable is only defined when compiling
    /// tests.
    fn test_env_helper(test_id: &str) -> TestEnvironment {
        TestEnvironment::new(
            test_id.to_string(),
            env!("CARGO_BIN_EXE_halo_remote"),
            env!("CARGO_BIN_EXE_halo_manager"),
        )
    }

    #[test]
    fn fencing() {
        let env = test_env_helper("fencing");

        let cluster = env.cluster();
        let host = cluster.hosts().next().unwrap();

        // First, make sure that the fence agent correctly reports that the remote is NOT yet
        // running:
        let powered_on = host.is_powered_on().unwrap();
        assert!(!powered_on);

        let _agent =
            env.start_remote_agents(vec![TestAgent::new(8004, Some("fence_mds00".to_string()))]);

        // Now, after starting the remote, the fence agent should report it is powered on:
        let powered_on = host.is_powered_on().unwrap();
        assert!(powered_on);

        // Fencing the agent OFF should succeed:
        host.do_fence(FenceCommand::Off).unwrap();

        // Now, start the agent again:
        let _agent =
            env.start_remote_agents(vec![TestAgent::new(8004, Some("fence_mds00".to_string()))]);

        // The remote agent should appear ON now:
        let powered_on = host.is_powered_on().unwrap();
        assert!(powered_on);
    }

    #[test]
    fn failover_partners() {
        let config_path = halo_lib::test_env::test_path("configs/failover.yaml");
        let cluster = halo_lib::cluster::Cluster::from_config(Some(config_path)).unwrap();

        let first_host = cluster.hosts().next().unwrap();
        let first_host_partner = Arc::clone(first_host);
        let partner_set_res = first_host.set_failover_partner(Some(first_host_partner));
        assert_eq!(
            partner_set_res,
            halo_lib::HandledResult::Err(halo_lib::HandledError {})
        );
    }
}
