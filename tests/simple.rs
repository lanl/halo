// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tokio::runtime::Runtime;

    use halo_lib::remote::ocf;
    use halo_lib::resource::{Location, Resource, ResourceStatus};
    use halo_lib::Buffer;

    use halo_lib::test_env::*;

    /// Create a TestEnvironment for a test.
    ///
    /// The path to the remote binary needs to be determined here and passed into the
    /// TestEnvironment constructor because the environment variable is only defined when compiling
    /// tests.
    fn test_env_helper(test_id: &str) -> TestEnvironment {
        TestEnvironment::new(test_id.to_string(), env!("CARGO_BIN_EXE_halo_remote"))
    }

    #[test]
    fn simple() {
        let mut env = test_env_helper("simple");

        let agent = TestAgent::new(halo_lib::remote_port(), None);

        let _agent = env.start_remote_agents(vec![agent]);

        let cluster = env.cluster(None);

        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            for res in cluster.resources() {
                assert_eq!(
                    res.start(Location::Home).await.unwrap(),
                    ocf::Status::Success
                );

                env.assert_agent_next_line(&agent_expected_line("start", res));

                let status = res.monitor(Location::Home).await.unwrap();
                assert_eq!(status, ocf::Status::Success);

                env.assert_agent_next_line(&agent_expected_line("monitor", res));

                assert_eq!(res.stop().await.unwrap(), ocf::Status::Success);
                env.assert_agent_next_line(&agent_expected_line("stop", res));
            }
        });
    }

    #[test]
    fn multi_agent() {
        let mut env = test_env_helper("multiagent");

        let _agents = env.start_remote_agents(vec![
            TestAgent::new(8001, Some("mds01".to_string())),
            TestAgent::new(8002, Some("oss01".to_string())),
        ]);

        let cluster = env.cluster(None);

        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            for res in cluster.resources() {
                assert_eq!(
                    res.start(Location::Home).await.unwrap(),
                    ocf::Status::Success
                );

                env.assert_agent_next_line(&agent_expected_line("start", res));

                let status = res.monitor(Location::Home).await.unwrap();
                assert_eq!(status, ocf::Status::Success);

                env.assert_agent_next_line(&agent_expected_line("monitor", res));

                assert_eq!(res.stop().await.unwrap(), ocf::Status::Success);
                env.assert_agent_next_line(&agent_expected_line("stop", res));
            }
        });
    }

    #[test]
    fn recover() {
        let test_id = "recover";
        let mut env = test_env_helper(&test_id);

        // Start an agent
        let _agent = env.start_remote_agents(vec![TestAgent::new(8003, None)]);

        // Get a Cluster structure with a shared management context:
        let mut context = env.manager_context();
        let mgr_stream = Buffer::new();
        context.out_stream = halo_lib::LogStream::Buffer(mgr_stream);
        let context = Arc::new(context);
        let cluster = env.cluster(Some(Arc::clone(&context)));

        // start a manager who shares the management context with this test:
        env.start_manager(Arc::clone(&context));

        let resources: Vec<&Resource> = cluster.resources().collect();

        // Check that all resources appear stopped
        for res in &resources {
            env.assert_manager_next_line(
                &context,
                &res.status_update_string(ResourceStatus::Unknown, ResourceStatus::Stopped),
            );
        }
        // Check that all resources appear running normally
        for res in &resources {
            env.assert_manager_next_line(
                &context,
                &res.status_update_string(ResourceStatus::Stopped, ResourceStatus::RunningOnHome),
            );
        }
        // Check that failing over resources works properly
        for res in &resources {
            env.stop_resource(&res);
            env.assert_manager_next_line(
                &context,
                &res.status_update_string(ResourceStatus::RunningOnHome, ResourceStatus::Stopped),
            );
            env.assert_manager_next_line(
                &context,
                &res.status_update_string(ResourceStatus::Stopped, ResourceStatus::RunningOnHome),
            );
        }
    }
}
