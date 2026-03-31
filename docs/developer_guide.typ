#set document(
  title: [HALO Developer Guide],
  author: "Thomas Bertschinger",
)

#title()

#set heading(numbering: "1.")
#outline()

#show link: underline
#show link: set text(fill: blue)

#let codeblock(body) = {
  set align(center)
  rect(
    inset: 8pt,
    radius: 4pt,
    body,
  )
}

#show raw.where(block: false): set text(fill: rgb("#054482"))

= Command Binaries

The entrypoints for the command binaries are in `src/bin/`.
The files in `src/bin/` just hold stubs that call into the library code that actually implements the commands.
- The implementation of the manager service starts in `src/manager/mod.rs`.
- The implementation of the remote service starts in `src/remote/mod.rs.`
- The implementations of the admin CLI commands live in `src/commands/`.

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

= HTTP API

HALO uses an HTTP API to communicate between the admin CLI utility and the manager service.
The client side implementation uses the
#link("https://docs.rs/reqwest/latest/reqwest/")[reqwest]
library, and the serve side uses the
#link("https://docs.rs/axum/latest/axum/")[axum]
library.

Communication occurs over a unix domain socket, rather than a TCP socket.

`curl` can be used to make requests of the manager service
for testing and debugging purposes:
#codeblock[
  ```bash
  $ curl --unix-socket halo.socket http://localhost/status | jq .
    % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current
                                   Dload  Upload   Total   Spent    Left  Speed
  100   787  100   787    0     0   509k      0 --:--:-- --:--:-- --:--:--  768k
  {
    "resources": [
      {
        "id": "test_zpool_00",
        "kind": "heartbeat/ZFS",
        "parameters": {
          "pool": "test_zpool_00"
        },
        "status": "Running",
        "comment": null,
        "managed": true
      },
      ...
  ```
]

= Key Data Structures

The fundamental runtime data structures in HALO are `Cluster`, `ResourceGroup`, `Resource`, and `Host`.

- `struct Cluster` is the overall container that holds the runtime state of a cluster under management.
  It holds references to the `ResourceGroup` and `Host` objects.

- `struct Host` represents a cluster node that hosts services.
  A `Host` holds the address of that cluster node.
  Since `Host`s are generally arranged in failover pairs, each `Host` has a reference
  to another `Host` object representing its partner.
  This reference is an `Option`, however, since some clusters don't use failover pairs.

- `struct ResourceGroup` represents a dependency tree of `Resource`s.
  It contains the `Resource` that represents the root of a tree, for example,
  a zpool in a typical Lustre use case.
  The only additional data that needs to be tracked for a `ResourceGroup` is
  whether the `ResourceGroup` is managed (whether HALO can actively start, stop, or migrate it).

- `struct Resource` is a single resource that is managed or monitored by HALO.
  A `Resource` holds the information needed by the OCF resource agent API
  to manage it, i.e., the name of the script as well as the parameter keys and values
  used for that particular resource.
  Since `Resource`s form a dependency tree, each `Resource` holds a list of its dependents.
  `Resource`s also have a reference to their home `Host` object,
  as well as the failover `Host` if applicable.


= How the HALO manager uses concurrency

The HALO manager process is written in async Rust. Async Rust represents concurrent tasks
as state machines that are cooperatively scheduled on OS threads.
The state machines that represent tasks are "lazy"--they are only evaluated
when actively polled (either implicitly through the `tokio` runtime for `tokio` tasks,
or explicitly through methods on "future combinators" like `FuturesUnordered`).

== Single-threaded manager

The HALO manager is (mostly) single-threaded--all async tasks run on the same OS thread.
Of course they cannot run simultaneously on the same thread, but tasks will yield at `await`
points to allow other tasks to run.

This means that it is critically important that a manager task in HALO must #strong[never] block
for an indefinite period of time (for example by performing blocking filesystem or network IO).
Occasionally a task will have to do something that must be done in a blocking way--in
such cases, the correct way to handle this is to use `tokio`'s `spawn_blocking()`
method to run such tasks in a separate thread pool, without blocking the main thread.
This explains the "mostly" qualifier in the statement that the manager is single-threaded.
All tasks run in a single thread, except those that would block, which run in a separate thread pool.

See `Cluster::write_record_nonblocking()` in `src/cluster.rs` as an example
of how to properly run blocking code within the HALO manager.

The single-threaded nature of the HALO manager,
combined with async Rust's cooperative evaluation of tasks,
is an important part of how the manager is designed.
These facts, when properly understood, make reasoning about concurrency relatively simple.
If a function never `await`s, it is never interrupted (because async Rust uses cooperative scheduling)
and no other task can run simultanesouly with it either (because the manager is single threaded).
This means that you never have to worry about the impact of concurrent code
when reading or writing such a function,
because there is no way for any code to run concurrently with it.

For example, consider the method `admin_fence_request()` defined in `src/host/ha/manage.rs`.
This method checks whether `state.outstanding_resource_tasks` is empty and
later iterates over the same collection.
One could be concerned that the collection could be modified between the initial empty check
and the subsequent iteration, but because this function does not `await`,
we know that there is no way for any other code to run concurrently and modify the collection.

== Use of multi-threaded synchronization types

In theory, a benefit of running on a single thread is that multithreaded synchronization
objects like `Arc` and `Mutex` do not need to be used.
Instead, `Rc` can be used for shared ownership
and `RefCell` can be used for interior mutability.

This benefit is difficult to realize in HALO because
HALO uses the `axum` library to implement the manager's HTTP server.
`axum` requires that objects passed to the `axum::serve()` method
(called in `src/manager/http.rs`)
implement `Send + Sync`, meaning that they must be thread-safe.
This requirement holds even though we use `axum` within a single-threaded context,
so the objects passed to `axum::serve()` will never actually be accessed
from multiple threads.

Thus, any of the objects in `HALO` that are reachable from the arguments passed
to `axum::serve()` must be thread-safe.
Specifically `Cluster`, and the objects reachable from it--`Host`, `ResourceGroup`,
and `Resource`--must be `Send + Sync`.

The practical takeaway of this is that anywhere that interior mutability is
required in any of those objects, a `Mutex` must be used, even though
it would seem like a a `RefCell` should be sufficient.
Similarly, anywhere that multiple ownership is needed, an `Arc` must be used
even though an `Rc` would seem sufficient.

This doesn't really matter much--the performance cost of the multi-threaded
synchronization types is irrelevant for HALO.
However, don't let the presence of `Arc`s and `Mutex`es fool you--the HALO
manager is single-threaded, even though it uses multi-threaded types.


== Structured Concurrency

Tasks in the HALO manager process can be divided into two categories based
on the kind of concurrency used to manage them.
Structured concurrency means using program syntax to constrain the lifetime of a task.
That is, a task's lifetime is bounded by some lexical scope.
Unstructured concurrency does not provide such a bound.

The following table contrasts structured and unstructured concurrency:

#[
  #table(
    columns: 2,
    table.header[*Untructured*][*Structured*],
    [`tokio::task::spawn()`, `tokio::task::spawn_blocking()`], [`futures::join!()`, `FuturesUnordered`, etc.],
    [Lifetime of task is unbounded], [Lifetime of task is bounded by a lexical scope],
    [Tasks cannot take non-`static` references], [Tasks can take references],
  )
]

Structured concurrency is preferred whenever possible.
The reason is that structured concurrency works well with Rust's lifetime analysis.
Because the lifetime of a task is constrained by a lexical scope,
the Rust compiler can put a bound on it.
This allows such structured tasks to take (non-`static`) references as arguments.

Unstructured tasks, on the other hand, do not have their lifetime bounded
and as such cannot take non-`static` references.
This makes their usage more difficult.

For example, consider the method `Cluster::write_record_nonblocking` defined in `src/cluster.rs`.
It needs to take an owned reference to a `Cluster`: `Arc<Cluster>`.
The only reason this is necessary is because a `&Cluster` cannot be passed to the
unstructured task in `tokio::task::spawn_blocking()`.

This is an unfortunate limitation.
In order to avoid it, it is preferred to use structured concurrency APIs.

= How HALO Manages High Availability Clusters

The most complicated part of HALO is the code that performs
active management of resources in an HA cluster with failover pairs.
This code is in `src/host/ha/manage.rs`.

Since this is the most complex part of HALO, and since any bugs in this code
could result in service outages or double-started resources,
I will attempt to explain how it works in detail in this section.

== Mutable state

`HostState`, defined in `host/ha/manage.rs`, holds mutable state for the Host.
This object's fields are mostly containers that hold `ResourceToken`s.
Such fields include, for example, `manage_these_resources` and `resources_in_transit`.

=== ResourceToken

A `ResourceToken` represents a single resource, like a zpool or Lustre mount.
`ResourceToken`s are used to ensure that each resource is only managed in one way at
a given time, and also that no resource is "forgotten about", never being managed anywhere.

When the HALO manager starts up, a single `ResourceToken` is created for each resource group
in the cluster.
(The name `ResourceToken` is slightly misleading because tokens correspond to `ResourceGroup`
and not `Resource`, but I thought `ResourceGroupToken` would be too long....)
This occurs in `Host::mint_resource_tokens()`.

`ResourceToken`s can never be destroyed once created.
To ensure this property is met, the destructor for `ResourceToken` will panic.
Ideally this property could be checked at compile time instead of run time.
If Rust had linear types, this would be possible.
Without linear types, a runtime check is the best we can do.

As resources go through failovers, migrations, being started and stopped, etc.,
you can track the resource through the HALO manager process
by looking at how the `ResourceToken` is passed around.

For example, when a resource undergoes management on a particular host,
the method `Host:manage_resource_group()` takes its `ResourceToken` as a parameter.
If that host undergoes fencing, the `ResourceToken` will be transferred into
the `resources_in_transit` field of `HostState`, and eventually be sent to
the task representing the partner host.

== Task Hierarchy

The manager launches an async task for each `Host` in the cluster.
When a `Host` task wants to manage a particular `ResourceGroup`, it will launch
a task for that `ResourceGroup`.
That `ResourceGroup` task can be thought of as a child of the `Host` task.
Those resource management tasks run the routine `Host::manage_resource_group()`.

=== Cooperative Cancellation Mechanism

A `Host` task can decide that management of a resource should no longer proceed.
Probably the most common reason for this is that fencing is deemed necessary.
In that case, any child resource management tasks need to exit.
In order to allow the `Host` task to cancel its child tasks,
a cooperative cancellation mechanism is used.

This mechanism uses the `ResourceTaskCancel` object.
An instance of this object is created every time a task running
`Host::manage_resource_group()` is launched.
This object is passed to that child task, and the `Host` management task keeps a
copy of it.
The `Host`'s copy is stored in the `oustanding_resource_tasks` field
of `HostState`.

The resource management task uses `tokio::select!` to listen for
the notification in the `ResourceTaskCancel` object concurrently
with performing resource management duties.
When the `ResourceTaskCancel` is triggered by the parent `Host` task,
the resource task will return.
It will give back the `ResourceToken` when it returns,
which will allow the `Host` task to move that `ResourceToken` to the appropriate state.

== Message Passing

`Host` tasks communicate with their failover partners using a message-passing style.
The `Host` object has `sender` and `receiver` fields that are used for message passing.
Each `Host` task listens on the `receiver`.
If a `Host` needs to transfer control of a `ResourceGroup` to its partner,
for example, because the host was fenced,
it uses the `sender` field on the partner object.
The message that is sent will include the `ResourceToken`,
which will allow the partner `Host` to begin management of the `ResourceGroup`.

In addition to failover-pair communication, a `Host` task also listens
for commands that come from the command-line utility.
Commands are handled by the HTTP server task in the manager process.
When a command requires taking action on some resource, like stopping a resource,
the HTTP server informs the `Host` task by sending a message using the `sender` field.
This happens in the method `Host::command()`.

== How a Host manages its tasks

The `Host` management task uses a `FuturesUnordered` object
to hold its child resource management tasks.
These tasks are created and ran in `Host::remote_connected_loop()`.
`FuturesUnordered` is used because it allows tasks to dynamically enter
and exit the set of child tasks that a `Host` is managing.

The tasks that are held in a `FuturesUnordered` object are only advanced
when that object is being polled.
The object is polled in its `next()` method.
Polling of the tasks occurs in a `while` loop:

#codeblock[
  ```Rust
  while let Some(event) = tasks.next().await {
      // handle event...
  }
  ```
]

Because child tasks are only polled in `tasks.next()`,
no progress will be made on the tasks during the body of the `while` loop.
For that reason, it is important that the `while` loop body executes quickly.
In particular it should not `await`; if it did so, all of the child resource tasks
could be delayed for an arbitrarily long time.

== Remote node is disconnected

When a remote node is disconnected, a `Host` task cannot manage resources on that node.
The `Host` task uses a different method to respond to events and messages
based on whether the remote node is connected or disconnected.
These methods are called `remote_connected_loop()` and `remote_disconnected_loop()`.

Before a `Host` task is allowed to exit `remote_connected_loop()`,
it must ensure that all child resource management tasks have exited.

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
