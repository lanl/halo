// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{fs, io, io::Write, net, sync::Arc};

use crate::{cluster::Cluster, manager::MgrContext, resource::Resource};

/// Given a relative `path` in the test directory, prepend the
/// full path to the test directory.
fn test_path(path: &str) -> String {
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

    /// For tests that use the manager,  this will store a file used for log output from the
    /// manager. The output in this file is for reference only; it is not used by the test
    /// itself.
    manager_log_file: Option<fs::File>,
}

impl TestEnvironment {
    /// Set up an environment for a test named `name`.
    ///
    /// Creates a specific unique subdirectory for the test and sets up the necessary environment
    /// variables for the remote agents.
    pub fn new(test_id: String, agent_binary_path: &str) -> Self {
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
            manager_log_file: None,
        }
    }

    /// Build a MgrContext for the given test environment. This assumes that the config file for
    /// the test is in a toml file named {test_id}.toml.
    pub fn manager_context(&self) -> MgrContext {
        let config_path = test_path(&format!("{}.toml", self.test_id));
        let socket_path = format!("{}/{}", self.private_dir_path, "test.socket");
        MgrContext::new(crate::commands::Cli {
            config: Some(config_path),
            socket: Some(socket_path),
            verbose: true,
            mtls: false,
            manage_resources: true,
            command: None,
        })
    }

    /// Build a Cluster for the given test environment.
    ///
    /// The test can optionally provide a MgrContext -- this would be used when the test wants
    /// to receive information from the running Manager thread via reading from the buffer in the
    /// shared MgrContext. If the caller does not provide a MgrContext, then a default context is
    /// used.
    pub fn cluster(&self, context: Option<Arc<MgrContext>>) -> Cluster {
        let context = context.unwrap_or(Arc::new(self.manager_context()));
        Cluster::new(context).unwrap()
    }

    /// Spawn a manager in a new thread. The manager will use a shared MgrContext so that the test
    /// that spawns it can receive communications from it.
    pub fn start_manager(&mut self, mgr_context: Arc<MgrContext>) {
        let manager_log_path = format!("{}/{}", self.private_dir_path, "manager.log");
        let mgr_log = std::fs::File::create(&manager_log_path)
            .expect(&format!("failed to open file: '{manager_log_path}'"));
        self.manager_log_file = Some(mgr_log);

        let cluster = Cluster::new(Arc::clone(&mgr_context))
            .expect("Could not create cluster from config file");

        std::thread::spawn(move || {
            if crate::manager::main(cluster).is_err() {
                std::process::exit(1);
            }
        });
    }

    /// Given a handle to the manager process's stdout/stderr, an output file to write to, and a
    /// comparison string slice, assert the given output is equivalent to the string slice's content.
    ///
    /// Panics if self.start_manager() has not previously been called.
    pub fn assert_manager_next_line(
        &self,
        context: &Arc<crate::manager::MgrContext>,
        expected_str: &str,
    ) {
        let mut logfile = self.manager_log_file.as_ref().unwrap();
        let mut buffer: Vec<u8> = vec![0u8; 4096];
        let n = context
            .out_stream
            .readln(&mut buffer)
            .expect("failed to read from reader");
        let _ = logfile
            .write(&buffer[0..n])
            .expect("failed to write to logfile");
        // `n-1` to strip the newline from `readln()`
        assert_eq!(
            &std::str::from_utf8(&buffer[0..n - 1]).unwrap(),
            &expected_str
        );
    }

    /// Starts a remote agent in a new process for each port in the given list of `ports`.
    ///
    /// Waits until the remotes are listening and ready to accept connections before returning, so
    /// that any subsequent code knows the remotes are up and ready.
    pub fn start_remote_agents(&self, mut agents: Vec<TestAgent>) -> Vec<ChildHandle> {
        let handles = agents
            .iter()
            .map(|agent| ChildHandle {
                handle: std::process::Command::new(&self.agent_binary_path)
                    .args(vec![
                        "--test-id",
                        &agent.id.as_ref().unwrap_or(&self.test_id),
                    ])
                    .env("HALO_TEST_LOG", &self.log_file_path)
                    .env("HALO_TEST_DIRECTORY", &self.private_dir_path)
                    .env("OCF_ROOT", test_path("ocf_resources"))
                    .env("HALO_NET", "127.0.0.0/24")
                    .env("HALO_PORT", format!("{}", agent.port))
                    .spawn()
                    .expect("could not launch process"),
            })
            .collect();

        let mut counter = 20;
        while !agents.is_empty() && counter > 0 {
            // Try to connect to each port; when connecting to one succeeds, remove it from the list
            // but keep trying the others.
            agents.retain(|agent| {
                let addr: net::SocketAddr = format!("127.0.0.1:{}", agent.port).parse().unwrap();
                match net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(50)) {
                    Ok(_) => false,
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => true,
                    Err(e) => {
                        panic!("Unexpected error attempting to connect to agent at {addr}: {e}")
                    }
                }
            });

            std::thread::sleep(std::time::Duration::from_millis(50));
            counter -= 1;
        }

        handles
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

    /// Stop over a given resource.
    ///
    /// Simulates a resource stopping by removing the state file that the test OCF resource
    /// script checks to determine if the resource is running.
    // TODO: once test agents are ran with an agent-specific ID, instead of a test ID that applies
    // to the entire test, then this should be updated to use that agent-specific ID in the
    // statefile name instead of the test ID. (the test ID will continue to be used for the
    // directory name.)
    pub fn stop_resource(&self, resource: &Resource) {
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
        let path = format!("{}.{}", self.test_id, path);
        let path = test_path(&format!("test_output/{}/", self.test_id)) + &path;
        std::fs::remove_file(&path).expect(&format!("failed to remove file '{}'", &path));
    }
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
