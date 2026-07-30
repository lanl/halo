// SPDX-License-Identifier: MIT
// Copyright 2026. Triad National Security, LLC.

use std::collections::HashMap;

use crate::{
    commands,
    config::{self, Config},
    manager::http,
    HandledResult,
};

use super::*;

/// Holds state related to a single HA test.
pub struct HaEnvironment {
    env: TestEnvironment,
    ports: [u16; 2],
    test_id: String,
    config: Config,
}

impl HaEnvironment {
    pub fn new_ha(test_id: String, agent_binary_path: &str, manager_binary_path: &str) -> Self {
        let ports = get_ports();
        let env = TestEnvironment::new("ha", &test_id, agent_binary_path, manager_binary_path);
        let config = ha_config(ports, test_id.clone());
        env.write_out_config(&config);
        Self {
            env,
            test_id,
            ports,
            config,
        }
    }

    pub fn start_agent(&self, which_one: usize) -> ChildHandle {
        let agent = TestAgent {
            port: self.ports[which_one],
            id: Some(self.agent_id(which_one)),
        };

        self.env
            .start_remote_agents(vec![agent])
            .into_iter()
            .next()
            .unwrap()
    }

    pub fn agent_id(&self, which_one: usize) -> String {
        format!("{}_{}", self.test_id, which_one)
    }

    pub fn start_manager(&self, manage_resources: bool) -> ManagerHandle {
        self.env.start_manager(manage_resources)
    }

    pub fn restart_manager(&self, old_manager: ManagerHandle) -> ManagerHandle {
        drop(old_manager);

        self.env.start_manager(true)
    }

    pub fn socket_path(&self) -> String {
        self.env.socket_path()
    }

    pub fn get_resource_by_id(&self, resource_id: &str) -> &config::Resource {
        for res in &self.config.resources {
            if res.name == resource_id {
                return res;
            }
        }

        panic!("Unable to find resource with id {resource_id}");
    }

    pub fn start_resource(&self, resource_id: &str, which_agent: usize) {
        self.env
            .start_resource(self.get_resource_by_id(resource_id), which_agent);
    }

    pub fn stop_resource(&self, resource_id: &str, which_agent: usize) {
        self.env
            .stop_resource(self.get_resource_by_id(resource_id), which_agent);
    }

    pub fn manage_resource(&self, resource_id: &str) {
        commands::manage::send_command(Some(&self.socket_path()), resource_id, true, None).unwrap();
    }

    pub fn unmanage_resource(&self, resource_id: &str) {
        commands::manage::send_command(Some(&self.socket_path()), resource_id, false, None)
            .unwrap();
    }

    pub fn failback(&self, onto: usize) -> HandledResult<()> {
        commands::failback::do_failback(
            Some(&self.socket_path()),
            &commands::failback::FailbackArgs {
                hostname: self.agent_id(onto),
                reason: Some("failback in test environment".to_string()),
            },
        )
    }

    pub fn fence(&self, which_one: usize, force: bool) -> HandledResult<()> {
        commands::fence::do_fence(
            Some(&self.socket_path()),
            &commands::fence::FenceArgs {
                hostname: self.agent_id(which_one),
                force,
                reason: Some("fence in test environment".to_string()),
            },
        )
    }

    pub fn activate_host(&self, which_one: usize) {
        commands::activate::do_activate(
            Some(&self.socket_path()),
            &self.agent_id(which_one),
            Some("activate in test environment".to_string()),
            true,
        )
        .unwrap();
    }

    pub fn deactivate_host(&self, which_one: usize) {
        commands::activate::do_activate(
            Some(&self.socket_path()),
            &self.agent_id(which_one),
            Some("deactivate in test environment".to_string()),
            false,
        )
        .unwrap();
    }

    pub fn reset_host(&self, which_one: usize) {
        commands::reset::do_reset(
            Some(&self.socket_path()),
            &commands::reset::ResetArgs {
                hostname: self.agent_id(which_one),
                reason: Some("reset in test environment".to_string()),
            },
        )
        .unwrap();
    }

    fn get_status(&self) -> http::ClusterJson {
        let status = commands::status::get_status(Some(&self.socket_path())).unwrap();
        eprintln!("{status:?}");
        status
    }

    #[track_caller]
    pub fn assert<const N: usize>(&self, assertions: [Assert; N]) {
        let status = self.get_status();
        for assertion in assertions {
            assertion.check(&status);
        }
    }
}

pub struct Assert {
    target: Target,
    what: AssertKind,
}

enum AssertKind {
    /// A resource status.
    Status(String),

    /// Whether a resource is managed.
    Managed(bool),

    /// Whether a host is connected.
    HostConnected(bool),

    /// Whether a host is fenced.
    HostFenced(bool),

    /// Whether a host is active.
    HostActive(bool),
}

pub enum Target {
    AllResources,
    ResourceGroup0,
    ResourceGroup1,
    Zpool0,
    Mdt0,

    Host0,
    Host1,
    AllHosts,
}

pub fn assert_status(target: Target, status: &str) -> Assert {
    if matches!(&target, Target::Host0 | Target::Host1 | Target::AllHosts) {
        panic!("Invalid to use a host target with assert_status(), must use a resource target.");
    }

    Assert {
        target,
        what: AssertKind::Status(status.to_owned()),
    }
}

pub fn assert_managed(target: Target, managed: bool) -> Assert {
    if matches!(&target, Target::Host0 | Target::Host1 | Target::AllHosts) {
        panic!("Invalid to use a host target with assert_managed(), must use a resource target.");
    }

    Assert {
        target,
        what: AssertKind::Managed(managed),
    }
}

pub fn assert_connected(target: Target, connected: bool) -> Assert {
    if !matches!(&target, Target::Host0 | Target::Host1 | Target::AllHosts) {
        panic!("Invalid to use a resource target with assert_connected(), must use a host target.");
    }

    Assert {
        target,
        what: AssertKind::HostConnected(connected),
    }
}

pub fn assert_fenced(target: Target, fenced: bool) -> Assert {
    if !matches!(&target, Target::Host0 | Target::Host1 | Target::AllHosts) {
        panic!("Invalid to use a resource target with assert_fenced(), must use a host target.");
    }

    Assert {
        target,
        what: AssertKind::HostFenced(fenced),
    }
}

pub fn assert_active(target: Target, active: bool) -> Assert {
    if !matches!(&target, Target::Host0 | Target::Host1 | Target::AllHosts) {
        panic!("Invalid to use a resource target with assert_active(), must use a host target.");
    }

    Assert {
        target,
        what: AssertKind::HostActive(active),
    }
}

impl Assert {
    #[track_caller]
    #[allow(clippy::collapsible_match, clippy::single_match)]
    fn check(&self, status: &http::ClusterJson) {
        for res in &status.resources {
            let (resource_status, _) = res.single_host_status();

            let check = match self.target {
                Target::AllResources => true,
                Target::ResourceGroup0 => res.id.contains("0"),
                Target::ResourceGroup1 => res.id.contains("1"),
                Target::Zpool0 => res.id == "zpool_0",
                Target::Mdt0 => res.id == "mdt_0",
                _ => false,
            };
            if check {
                match &self.what {
                    AssertKind::Status(status) => {
                        if resource_status != status {
                            panic!(
                                "Expected status '{}', got '{}', for resource '{}'",
                                status, resource_status, res.id
                            );
                        }
                    }
                    AssertKind::Managed(managed) => {
                        if &res.managed != managed {
                            panic!(
                                "Expected managed: {managed} but was {}, for resource {}",
                                res.managed, res.id
                            );
                        }
                    }
                    _ => {}
                }
            }
        }

        for host in &status.hosts {
            let check = match self.target {
                Target::Host0 => host.id.ends_with("0"),
                Target::Host1 => host.id.ends_with("1"),
                Target::AllHosts => true,
                _ => false,
            };

            if check {
                match &self.what {
                    AssertKind::HostConnected(conn) => {
                        if &host.connected != conn {
                            panic!(
                                "Expected connected: {conn} but was {}, for host {}",
                                host.connected, host.id,
                            );
                        }
                    }
                    AssertKind::HostFenced(f) => {
                        if &host.fenced != f {
                            panic!(
                                "Expected fenced: {f} but was {}, for host {}",
                                host.fenced, host.id,
                            );
                        }
                    }
                    AssertKind::HostActive(a) => {
                        if &host.active != a {
                            panic!(
                                "Expected active: {a} but was {}, for host {}",
                                host.active, host.id,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

impl Drop for HaEnvironment {
    /// When dropping the environment, make sure that no resources were "double-started"--that
    /// is, started on both hosts in a pair.
    fn drop(&mut self) {
        for resource in self.config.resources.iter() {
            if self.env.resource_is_started(resource, 0)
                && self.env.resource_is_started(resource, 1)
            {
                panic!("Resource {} was double-started!", resource.name)
            }
        }
    }
}

/// Creates an HA-pair config for use in the ha tests.
fn ha_config(ports: [u16; 2], test_id: String) -> Config {
    let mut config = Config {
        hosts: Vec::new(),
        resources: Vec::new(),
        resource_groups: Vec::new(),
    };

    for i in 0..2 {
        let zpool_name = || -> String { format!("zpool_{i}") };
        let lustre_name = || -> String { format!("mdt_{i}") };
        let my_hostname = || -> String { format!("127.0.0.1:{}", ports[i]) };
        let partner_hostname =
            || -> String { format!("127.0.0.1:{}", if i == 0 { ports[1] } else { ports[0] }) };

        let root_resource = config::Resource {
            name: zpool_name(),
            kind: "heartbeat/ZFS".to_string(),
            parameters: HashMap::from([("pool".to_string(), zpool_name())]),
            dependents: vec![lustre_name()],
        };

        let child_resource = config::Resource {
            name: lustre_name(),
            kind: "lustre/Lustre".to_string(),
            parameters: HashMap::from([
                ("mountpoint".to_string(), lustre_name()),
                ("target".to_string(), lustre_name()),
                ("kind".to_string(), "mdt".to_string()),
            ]),
            dependents: Vec::new(),
        };

        let host = config::Host {
            hostname: my_hostname(),
            fence_agent: Some("fence_test".to_string()),
            fence_parameters: Some(HashMap::from([
                ("target".to_string(), format!("{test_id}_{i}")),
                ("test_id".to_string(), format!("ha/{test_id}")),
            ])),
        };

        let resource_group = config::ResourceGroup {
            home_host: my_hostname(),
            failover_hosts: vec![partner_hostname()],
            root: zpool_name(),
        };

        config.hosts.push(host);
        config.resources.push(root_resource);
        config.resources.push(child_resource);
        config.resource_groups.push(resource_group);
    }

    config
}
