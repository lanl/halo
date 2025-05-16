# HALO Test Environment

The HALO test environment uses processes and threads running on one system to emulate a distributed
cluster. While a collection of multiple processes running on one host is not a perfect analogy for a
distributed cluster, the behavior can be similar enough to suitably test the HALO functionality. And
it has the benefit of making automated tests much simpler to implement and run.

## Resource State Files

In order to emulate the starting and stopping of resources, test agents create and remove a file
that represents a given resource. A file is used to represent the state of a resource because it
makes it easy to observe the current state of the cluster, as well as to interfere with that state
by either removing or creating the file. This means that initiating a change of state of a resource
can be done either by the resource agent, by the user, or by the test suite itself (to simulate a
resource crashing / failing).

## Remote Agents

### Launching Remote Agents

Remote agents are run as separate processes on the test host. Each remote agent listens on the
localhost IP address. Because all tests run concurrently--and within one test, multiple agents
may run--each remote agent must be assigned a unique port that does not collide with any other
test agent in the whole test framework.

Remote agents run with a test ID that is specified in each test, and generally should be the same
as the name of the test. This test ID is used to specify the location of the resource state files
used by the given agent.

For example, for the `simple` test, the remote agent has a test ID of `simple` and the state files
live in `tests/test_output/simple/`.

### Uniquely Identifying Remote Agents

When a test runs multiple agents (because the test is simulating a cluster with multiple nodes), the
test ID is not suitable to uniquely identify the agents. A new unique identifier for the agents is
needed for operations like fencing. When a test launches the agents, it can specify an optional
unique ID per-agent. This agent ID is encoded in the path to the resource state files managed
by that agent, so that the test environment can tell which agent "owns" a given resource at a
particular moment.

### Fencing Test Agents

In a production environment, fencing involves running a command which will launch IPMI or Redfish
commands over the network. In the test environment, however, fencing must work differently since
"remote" nodes are really represented as processes on the test host.

Powering off a node can be simulated by killing the remote agent process, and potentially removing
the resource state files for all of the resources that it owned.
