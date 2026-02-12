// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{fs, io, io::Write, net};

use crate::{cluster::Cluster, config, config::Config, manager, resource::Resource};

/// Given a relative `path` in the test directory, prepend the
/// full path to the test directory.
pub fn test_path(path: &str) -> String {
    std::env::var("CARGO_MANIFEST_DIR").unwrap() + "/tests/" + path
}

trait IgnoreEexist {
    fn ignore_eexist(self) -> Self;
}

impl IgnoreEexist for io::Result<()> {
    fn ignore_eexist(self) -> Self {
        match self {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// This struct is used to hold handles to the remote agent processes so that they can be shut
/// down when the test ends.
pub struct ChildHandle {
    pub handle: std::process::Child,
}

impl Drop for ChildHandle {
    fn drop(&mut self) {
        let _ = self.handle.kill();
    }
}

/// This struct is used to hold the handle to the manager service so that it can be shut down when
/// the test ends.
///
/// This needs different logic from the remote agent handle because when the manager shuts down, we
/// want to block until it actually stops - to be sure that the manager stops before the remote
/// agents.
pub struct ManagerHandle {
    handle: std::process::Child,
    socket_path: String,
}

impl Drop for ManagerHandle {
    fn drop(&mut self) {
        let _ = self.handle.kill();

        // Block until manager actually stops...
        let mut counter = 20;
        while counter > 0 {
            match std::os::unix::net::UnixStream::connect(&self.socket_path) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => return,
                Err(e) => panic!("Unexpected error wait for manager to stop: {e}."),
            }

            counter -= 1;
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        panic!("Manager service did not stop within a reasonable amount of time.");
    }
}

/// A TestEnvironment holds all the information needed to access a test's runtime state. This
/// includes a "private" working directory in which log files, resource status files, and other
/// state for the running test will be stored.
///
/// All access to the test's state on the filesystem should be done via methods on TestEnvironment
/// rather than coded in the tests themselves.
pub struct TestEnvironment {
    /// The name of the test, used to determine its private directory for holding test state.
    test_id: String,

    /// The path to this test's private working directory.
    private_dir_path: String,

    /// The path to the log file that is used by the test OCF resource agents (under
    /// `tests/ocf_resources/`) to log the actions they take.
    log_file_path: String,

    /// A handle to the OCF resource log file, used to determine what actions they take during the
    /// test run.
    log_file: fs::File,

    /// The agent binary path has to be passed in as an argument from the tests because the
    /// CARGO_BIN_EXE_* environment variables aren't defined during non-test compilation.
    agent_binary_path: String,

    manager_binary_path: String,
}

impl TestEnvironment {
    /// Set up an environment for a test named `name`.
    ///
    /// Creates a specific unique subdirectory for the test and sets up the necessary environment
    /// variables for the remote agents.
    pub fn new(test_id: String, agent_binary_path: &str, manager_binary_path: &str) -> Self {
        // Each test gets a "private" directory named after its test_id.
        let private_dir_path = test_path(&format!("test_output/{test_id}"));
        // Start by emptying out the test's private directory, so that files from a previous test
        // run don't impact this run:
        match std::fs::remove_dir_all(&private_dir_path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => panic!("Could not clean up test directory: {e}"),
        };

        let log_file_path = test_path(&format!("test_output/{test_id}/test_log"));

        std::fs::create_dir(test_path("test_output"))
            .ignore_eexist()
            .unwrap();

        std::fs::create_dir(&private_dir_path).unwrap();

        let _ = std::fs::File::create(&log_file_path).unwrap();
        // Since create() opens the file in write-only mode, ignore that handle and re-open a
        // read-only handle for the test's use:
        let log_file = std::fs::File::open(&log_file_path).unwrap();

        Self {
            test_id,
            private_dir_path,
            log_file_path,
            log_file,
            agent_binary_path: agent_binary_path.to_string(),
            manager_binary_path: manager_binary_path.to_string(),
        }
    }

    /// Writes out the given config as a yaml file in the tests private directory.
    pub fn write_out_config(&self, config: &Config) {
        let mut config_file =
            std::fs::File::create(format!("{}/config.yaml", &self.private_dir_path)).unwrap();
        let contents = serde_yaml::to_string(&config).unwrap();
        config_file.write_all(contents.as_bytes()).unwrap();
    }

    pub fn socket_path(&self) -> String {
        format!("{}/test.socket", &self.private_dir_path)
    }

    /// Build a MgrContext for the given test environment. This assumes that the config file for
    /// the test is in a yaml file named {test_id}.yaml.
    pub fn manager_args(&self) -> manager::Cli {
        let config_path = test_path(&format!("{}.yaml", self.test_id));
        let statefile_path = test_path("halo.state");
        let socket_path = format!("{}/{}", self.private_dir_path, "test.socket");
        manager::Cli {
            config: Some(config_path),
            socket: Some(socket_path),
            statefile: Some(statefile_path),
            mtls: false,
            verbose: false,
            manage_resources: true,
            fence_on_connection_close: true,
            sleep_time: 5000,
        }
    }

    /// Build a Cluster for the given test environment.
    ///
    /// The test can optionally provide a MgrContext -- this would be used when the test wants
    /// to receive information from the running Manager thread via reading from the buffer in the
    /// shared MgrContext. If the caller does not provide a MgrContext, then a default context is
    /// used.
    pub fn cluster(&self, args: Option<manager::Cli>) -> Cluster {
        let args = args.unwrap_or(self.manager_args());
        Cluster::new(args).unwrap()
    }

    /// Starts a remote agent in a new process for each port in the given list of `ports`.
    ///
    /// Waits until the remotes are listening and ready to accept connections before returning, so
    /// that any subsequent code knows the remotes are up and ready.
    pub fn start_remote_agents(&self, agents: Vec<TestAgent>) -> Vec<ChildHandle> {
        let handles = agents
            .iter()
            .map(|agent| {
                let log_file = match &agent.id {
                    Some(id) => format!("{}/agent_{id}_log", &self.private_dir_path),
                    None => format!("{}/agent_log", &self.private_dir_path),
                };
                let log_file = std::fs::File::create(log_file).unwrap();

                ChildHandle {
                    handle: std::process::Command::new(&self.agent_binary_path)
                        .args(vec![
                            "--verbose",
                            "--test-id",
                            &agent.id.as_ref().unwrap_or(&self.test_id),
                        ])
                        .env("HALO_TEST_LOG", &self.log_file_path)
                        .env("HALO_TEST_DIRECTORY", &self.private_dir_path)
                        .env("OCF_ROOT", test_path("ocf_resources"))
                        .env("HALO_NET", "127.0.0.0/24")
                        .env("HALO_PORT", format!("{}", agent.port))
                        .env("HALO_LOG", "trace")
                        .stderr(std::process::Stdio::from(log_file))
                        .spawn()
                        .expect("could not launch process"),
                }
            })
            .collect();

        for agent in agents {
            // Wait for connection to each agent to succeed.
            wait_for_service(&format!("127.0.0.1:{}", agent.port), true);
        }

        handles
    }

    /// Starts the manager in a new process for
    pub fn start_manager(&self, manage_resources: bool) -> ManagerHandle {
        let log_file = format!("{}/manager_log", &self.private_dir_path);
        let log_file = std::fs::File::create(log_file).unwrap();

        let socket_path = format!("{}/test.socket", &self.private_dir_path);
        let config_path = format!("{}/config.yaml", &self.private_dir_path);
        let statefile_path = format!("{}/halo.state", &self.private_dir_path);

        let mut args = vec![
            "--verbose",
            "--fence-on-connection-close",
            "--config",
            &config_path,
            "--statefile",
            &statefile_path,
            "--socket",
            &socket_path,
            "--sleep-time",
            "500",
        ];

        if manage_resources {
            args.push("--manage-resources");
        }

        let handle = std::process::Command::new(&self.manager_binary_path)
            .args(args)
            .stderr(std::process::Stdio::from(log_file))
            .spawn()
            .expect("could not launch manager process");

        wait_for_service(&socket_path, false);

        ManagerHandle {
            handle,
            socket_path,
        }
    }

    /// Reads a line from the shared file used for communication from the agent, and asserts that
    /// it equals the given expected `line`.
    pub fn assert_agent_next_line(&mut self, line: &str) {
        use io::{BufRead, Read};

        let mut buf = vec![0u8; 4096];

        let _n = self.log_file.read(&mut buf).unwrap();

        let contents = buf.lines().next().unwrap().unwrap();

        assert_eq!(contents, line);
    }

    /// Simulate a resource stopping by removing the state file that the test OCF resource
    /// script checks to determine if the resource is running.
    pub fn stop_resource(&self, resource: &config::Resource, agent: usize) {
        let path = self.get_resource_path(resource, agent);
        std::fs::remove_file(&path).expect(&format!("failed to remove file '{}'", &path));
    }

    /// Simulate a resource startin by creating the state file that the test OCF resource
    /// script checks to determine if the resource is running.
    pub fn start_resource(&self, resource: &config::Resource, agent: usize) {
        let path = self.get_resource_path(resource, agent);
        std::fs::File::create(&path).expect(&format!("failed to create file '{}'", &path));
    }

    /// Returns true if a resource is "started", meaning its state file exists for the given agent.
    pub fn resource_is_started(&self, resource: &config::Resource, agent: usize) -> bool {
        let path = self.get_resource_path(resource, agent);
        std::fs::exists(path).unwrap()
    }

    /// Get the path to the "resource state file" used in a test -- that is, the file whose
    /// presence indicates the resource is running and whose absence indicates it is stopped.
    fn get_resource_path(&self, resource: &config::Resource, agent: usize) -> String {
        let path = match resource.kind.as_str() {
            "heartbeat/ZFS" => &format!("zfs.{}", resource.parameters.get("pool").unwrap()),
            "lustre/Lustre" => &format!(
                "lustre.{}",
                resource
                    .parameters
                    .get("mountpoint")
                    .unwrap()
                    .replace("/", "_")
            ),
            _ => unreachable!(),
        };
        let path = format!("{}_{agent}.{}", self.test_id, path);
        test_path(&format!("test_output/{}/", self.test_id)) + &path
    }
}

/// Wait for the service (e.g., manager, remote agent) at the specified address to start.
/// TCP socket if tcp is true, else Unix socket.
///
/// Panics if too much time passes without the service starting.
fn wait_for_service(addr: &str, tcp: bool) {
    let mut counter = 20;
    while counter > 0 {
        if tcp {
            let addr: net::SocketAddr = addr.parse().unwrap();
            match net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(50)) {
                Ok(_) => return,
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {}
                Err(e) => {
                    panic!("Unexpected error attempting to connect to agent at {addr}: {e}")
                }
            }
        } else {
            match std::os::unix::net::UnixStream::connect(addr) {
                Ok(_) => return,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    panic!("Unexpected error attempting to connect to manager at {addr}: {e}")
                }
            }
        };

        std::thread::sleep(std::time::Duration::from_millis(200));
        counter -= 1;
    }

    panic!("Unable to connect to service at {addr} within a reasonable time.");
}

/// The information needed to launch a remote agent binary in the test environment.
pub struct TestAgent {
    /// The port must be unique across all tests, since all tests run concurrently and thus every
    /// remote agent in the test environment can try to listen on the localhost IP Address at the
    /// same time.
    pub port: u16,

    /// If an agent ID is specified here, it is passed to the agent binary as the `--test-id`
    /// instead of the test ID used for the whole test. This is for tests that run multiple remote
    /// agents, so that they can uniquely identify the agents. (Technically the port number could
    /// be used as a unique ID for the different agents, but it's not very meaningful, so this
    /// allows using a meaningful string as the unique ID.)
    pub id: Option<String>,
}

impl TestAgent {
    pub fn new(port: u16, id: Option<String>) -> Self {
        Self { port, id }
    }
}

/// Given an operation `op` and a resource `res`, formats the line that we expect to see in the
/// communication file for succesfully performing `op` on `res`.
pub fn agent_expected_line(op: &str, res: &Resource) -> String {
    match res.kind.as_str() {
        "heartbeat/ZFS" => format!("zfs {} pool={}", op, res.parameters.get("pool").unwrap()),
        "lustre/Lustre" => format!(
            "lustre {} mountpoint={} target={}",
            op,
            res.parameters.get("mountpoint").unwrap(),
            res.parameters.get("target").unwrap(),
        ),
        _ => unreachable!(),
    }
}

/// When a remote agent is running in the test environment, it may need to be fenced. In order to
/// enable fencing, the test fence program needs to know this agent's PID so that it can be killed
/// with a signal.
///
/// This knowledge is shared with the test fence program by writing this agent's PID to a file in a
/// location known to the test fence program. That location is:
///
///     `{private_test_directory}/{agent_id}.pid`
///
/// `private_test_directory` is typically `tests/test_output/{test_name}`, and this agent gets that
/// directory from the environment variable `HALO_TEST_DIRECTORY`.
///
/// `agent_id` passed as an optional argument.
///
/// This function only writes to a file when both `private_test_directory` and `agent_id` are
/// known. If one or both is not specified, it is assumed that the agent is either not running in
/// the test environment, or it is running in a test that does not use fencing and does not need
/// this shared information.
pub fn maybe_identify_agent_for_test_fence(args: &crate::remote::Cli) {
    let Some(ref agent_id) = args.test_id else {
        return;
    };

    let Ok(test_directory) = std::env::var("HALO_TEST_DIRECTORY") else {
        return;
    };

    // It is important that the remote agent NOT proceed if it is unable to write its PID to the
    // required file. This is because the test fence agent uses the presence or absence of this
    // file as a way to know whether a particular remote agent is "powered on". Thus, these
    // `unwrap()`s are needed to maintain the test environment's invariants.
    let pid_file = format!("{test_directory}/{agent_id}.pid");
    let mut file = std::fs::File::create(pid_file).unwrap();
    let me = format!("{}", std::process::id());
    file.write_all(me.as_bytes()).unwrap();
}
