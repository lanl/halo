// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::fs;
use std::io;
use std::io::Write;
use std::net;
use std::sync::Arc;

use crate::cluster::Cluster;
use crate::manager::MgrContext;
use crate::resource::Resource;

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

        // The test OCF resource agents check this environment variable to know what file to use to
        // log the actions they take.
        std::env::set_var("HALO_TEST_LOG", &log_file_path);

        // The halo_remote program needs this environment variable to know where to find the OCF
        // resource agent scripts.
        std::env::set_var("OCF_ROOT", test_path("ocf_resources"));

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
            if let Err(_) = crate::manager::main(cluster) {
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
    pub fn start_remote_agents(&self, mut ports: Vec<u16>, network: &str) -> Vec<ChildHandle> {
        let network = network.to_string();

        let handles = ports
            .iter()
            .map(|port| ChildHandle {
                handle: std::process::Command::new(&self.agent_binary_path)
                    .args(vec!["--test-id", &self.test_id])
                    .env("HALO_TEST_LOG", &self.log_file_path)
                    .env("HALO_TEST_DIRECTORY", &self.private_dir_path)
                    .env("HALO_NET", &network)
                    .env("HALO_PORT", format!("{port}"))
                    .spawn()
                    .expect("could not launch process"),
            })
            .collect();

        let mut counter = 20;
        while !ports.is_empty() && counter > 0 {
            // Try to connect to each port; when connecting to one succeeds, remove it from the list
            // but keep trying the others.
            ports.retain(|port| {
                let addr: net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
                match net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(50)) {
                    Ok(_) => false,
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => true,
                    Err(e) => panic!(
                        // "Unexpected error attempting to connect to agent at 127.0.0.1:{port}: {e}"
                        "Unexpected error attempting to connect to agent at {addr}: {e}"
                    ),
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
        let path = test_path(&format!("test_output/{}/", self.test_id)) + path;
        std::fs::remove_file(&path).expect(&format!("failed to remove file '{}'", &path));
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
