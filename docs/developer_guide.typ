#set document(
  title: [HALO Developer Guide],
  author: "Thomas Bertschinger",
)

#title()

#set heading(numbering: "1.")
#outline()

#show link: underline

= RPC Protocol

HALO uses a #link("https://capnproto.org/")[Cap'n Proto] RPC protocol to communicate between the manager daemon
and remote agent daemons.
The protocol schema is defined in the file `halo.capnp`.
The protocol is extremely simple, only supporting a single RPC called `operation()`.

The arguments to `operation()` are passed along to an OCF Resource Agent script which actually performs
the requested op.
`operation()` then returns the result:
- when running the OCF Resource Agent script fails, a string containing the error message
  (like "File not found...") is returned.
- when running the script succeeds, the integer exit status of the script is returned
  - a status of `0` indicates success.
  - a nonzero status indicates some kind of failure; in that case a string containing the
    standard error output from the script is included in the RPC reply.

= Key Data Structures

The fundamental runtime data structures in HALO are `Cluster`, `ResourceGroup`, `Resource`, and `Host`.

`struct Cluster` is the overall container that holds the runtime state of a cluster under management.

= Test Environment

The HALO test environment uses processes and threads running on one system to emulate a distributed
cluster. While a collection of multiple processes running on one host is not a perfect analogy for a
distributed cluster, the behavior can be similar enough to suitably test the HALO functionality. And
it has the benefit of making automated tests much simpler to implement and run.

== Resource State Files

In order to emulate the starting and stopping of resources, test agents create and remove a file
that represents a given resource. A file is used to represent the state of a resource because it
makes it easy to observe the current state of the cluster, as well as to interfere with that state
by either removing or creating the file. This means that initiating a change of state of a resource
can be done either by the resource agent, by the user, or by the test suite itself (to simulate a
resource crashing / failing).

== Remote Agents

=== Environment Variables

The remote agent needs to share state with the test runner program, and it does so via files whose
locations are denoted by environment variables.

- `HALO_TEST_DIRECTORY` - the private directory for all of the files used in a particular test. This
  is typically set to `tests/test_output/{test_name}`.
- `HALO_TEST_LOG` - the path to the shared log file that the OCF Resource Agent logs its actions to.
- `HALO_TEST_ID` - this is the unique ID for each agent within a test, needed when a single test
  runs multiple agents. This is used in the path to the resource state files so that the test
  environment can tell which of several test agents is currently hosting a resource. It is also
  used in the path to the agent's PID file so that each test agent can be uniquely identified by the
  test fencing program.
- `OCF_ROOT` - this tells the remote agent where to look for the OCF Resource Agent scripts, which
  live under `tests/ocf_resources`.

Because all the tests run concurrently in the same address space, the environment variables cannot
be used by the tests themselves: the information must be stored in the test-specific `TestEnvironment`
structure, or another private location.

=== Launching Remote Agents

Remote agents are run as separate processes on the test host. Each remote agent listens on the
localhost IP address. Because all tests run concurrently--and within one test, multiple agents
may run--each remote agent must be assigned a unique port that does not collide with any other
test agent in the whole test framework.

Remote agents run with an agent ID that is optionally specified in each test. If it is not
specified, the test-wide test ID is used. However, if a test runs multiple agents, the test ID would
not be unique, so a unique ID can be specified per-agent in that case. This agent ID is used to
specify the location of the resource state files used by the given agent.

For example, for the `simple` test, the remote agent has a test ID of `simple` and the state files
live in `tests/test_output/simple/`.

=== Uniquely Identifying Remote Agents

When a test runs multiple agents (because the test is simulating a cluster with multiple nodes), the
test ID is not suitable to uniquely identify the agents. A new unique identifier for the agents is
needed for operations like fencing. When a test launches the agents, it can specify an optional
unique ID per-agent. This agent ID is encoded in the path to the resource state files managed
by that agent, so that the test environment can tell which agent "owns" a given resource at a
particular moment.

=== Fencing Test Agents

In a production environment, fencing involves running a command which will launch IPMI or Redfish
commands over the network. In the test environment, however, fencing must work differently since
"remote" nodes are really represented as processes on the test host.

Powering off a node can be simulated by killing the remote agent process, and potentially removing
the resource state files for all of the resources that it owned.

Being able to "power off" a test agent requires knowing its PID. A test agent shares its PID by
writing it to a file in a known location (see the function `maybe_identify_agent_for_test_fence()`).
This location is determined by two pieces of information: the test's private directory, and the
unique agent ID.

Being able to "power on" a test agent requires storing the new PID somewhere so that it can be known
when it next needs to be fenced.

== Manager

Some tests don't use the manager at all and directly call the methods on `Resource` to start, stop,
and monitor resources. Other tests launch the manager as a separate thread in the test process.


== How to Test Fencing by Hand

To test fencing by hand, use the failover config at `tests/failover.yaml`. This config defines two
hosts that are in a failover pair, and which use the test fence agent.

1. Launch one or both of the test agents:

```bash
$ HALO_TEST_DIRECTORY=tests/test_output/failover OCF_ROOT=tests/ocf_resources/ ./target/debug/halo_remote --network 127.0.0.0/24 --port 8005  --test-id fence_mds00
$ HALO_TEST_DIRECTORY=tests/test_output/failover OCF_ROOT=tests/ocf_resources/ ./target/debug/halo_remote --network 127.0.0.0/24 --port 8006  --test-id fence_mds01
```

(Note that `HALO_TEST_DIRECTORY` must be defined as shown above for fencing to work, because the
test fence agent at `tests/fence_test` is hardcoded to assume that the remote PID file is under
`tests/test_output/{test_id}`.)

2. Run the manager service:

```bash
./target/debug/halo_manager --config tests/failover.yaml --socket halo.socket  --manage-resources --verbose
```

3. Run `power status` to confirm that the fence agent is able to check the status of each remote:

```bash
./target/debug/halo --config tests/failover.yaml  power status
```

4. Run `power off` to try killing a remote agent, and see how the manager responds:

```bash
./target/debug/halo --config tests/failover.yaml  power off fence_mds00
```
