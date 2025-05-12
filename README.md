# HALO (formerly GoLustre)

HALO, or High Availability Low Overhead, is a cluster management system designed for managing Lustre HA and similar use cases.
HALO was previously known as GoLustre, and only supported Lustre HA, but now it can manage other cluster types.

LANL software release number: O4905.

## Quick Start using an example config:

1. Start the remote service, giving it an ID of `test_agent` (the test ID is used to control its resources in the test environment):

```bash
$ OCF_ROOT=tests/ocf_resources/ ./target/debug/halo_remote --network 127.0.0.0/24 --port 8000  --test-id test_agent
```

2. Start the manager service, using `--manage-resources` to tell it to actively manage resources:

```bash
$ ./target/debug/halo --config tests/simple.toml --socket halo.socket  --manage-resources --verbose
```

You should see it output information about updating the state of resources.

The test environment uses the existence of empty files as a sign that a resource is "running".
Look in the halo directory for files named `test_agent.*` -- these are created when the test agent "starts" a resource.

3. Run the `status` command:

```bash
$ ./target/debug/halo --config tests/simple.toml --socket halo.socket  status
```

This outputs information on the state of the resources at the current moment.

4. Try "stopping" a resource by removing its state file:

```bash
$ rm test_agent.lustre._mnt_test_ost
```

You should see the manager process output status changes as it notices the resource is stopped, and then starts the resource. Try running the monitor command quickly multiple times as the resource state changes, to see if you can catch it in various states.

## Architecture

HALO consists of two services: a management service that runs on the cluster master node, and a remote service that runs on Lustre servers.
The management service has the logic on where and when to start/stop resources. The remote service is "dumb" and only responds to commands from the manager.
The operator uses the CLI to interact with the management service on the master node.

### Management Service

The management service uses the `halo` binary. The entry point is in `src/bin/manager.rs`, and the functionality is in `src/manager.rs`.

The manager launches two threads of control.

- The first is a server, launched in `src/manager.rs:server_main()` which listens for commands from the command line utility, and responds to them.

- The second is the actual manager process, launched in `src/manager.rs:manager_main()`, which periodically launches monitor commands to the remote services to monitor the status of the resources that they host.

### Remote Service
The remote service uses the `halo_remote` binary. The entry point is in `src/bin/remote.rs` and the functionality is in `src/remote/*.rs`. 

The remote agent runs a capnp RPC server whose main loop is in `src/remote/mod.rs:__agent_main()`. The agent listens for requests from the manager and acts on them.
The requests are to stop, start, or monitor a resource.
Which resource to act on is determined by the arguments passed in the request from the manager.
The arguments determine the location of the OCF Resource Agent script that is used to actually process the requests.

## Installation

To install and start the management server:
```bash
# cp systemd/halo.service /lib/systemd/system/
# cp target/debug/halo /usr/local/sbin/
# systemctl start halo.service
```

To install and start the remote server:
```bash
# clush -g mds,oss --copy systemd/halo_remote.service --dest /lib/systemd/system/
# clush -g mds,oss --copy target/debug/halo_remote --dest /usr/local/sbin/
# systemctl start halo-remote.service
```

## Configuration

The daemon can be configured via environment variables defined in `/etc/sysconfig/halo`. HALO recognizes the following variables:

- `HALO_CONFIG` -- defines the location to search for the configuration file (default: `/etc/halo/halo.conf`).
- `HALO_PORT` -- defines port for the daemon to listen on (default `8000`).
- `HALO_NET` -- defines the network that the daemon listens on (default `192.168.1.0/24`).

When using TLS, HALO additionally will check `HALO_{CLIENT,SERVER}_{CERT,KEY}`.

## Code Layout

- `src/lib.rs`: defines a few helper functions, the default values for the config file, socket, etc., and is the root for the code shared by the binaries.

- `src/halo_capnp.rs`: the generated capnp RPC code is imported here.
  This module also defines helper functions to make RPC calls to reduce boilerplate for users of the RPC interface.

- `src/config.rs`: holds the config object which is used for the cluster configuration file.

- `src/cluster.rs`: holds the data structure that represents a cluster's in-memory state.
  `Cluster::main_loop()` is the main entrypoint for the cluster management server.

- `src/resource.rs`: holds the data structures that represent resources: `ResourceGroup` represents a dependency tree of `Resource`s.
   The lifecycle of a resource group is started in `ResourceGroup::main_loop()`.

- `src/host.rs`: holds the data structures for representing a host's state. Also includes the fencing / power management implementation.

- `src/manager.rs`: the code for the manager server (which kicks off the resource lifecycle code), and the CLI server, which responds to requests from the command line.
