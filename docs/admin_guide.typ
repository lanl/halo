#set document(
  title: [HALO System Administrator Guide],
  author: "Thomas Bertschinger",
)

#title()

== Abstract

HALO is a high-availability system designed for the management of distributed filesystems.
It starts and stops resources, and performs power management of servers,
with the aim of ensuring filesystem availability.
HALO is targeted specifically at the Lustre filesystem,
and is designed to be as simple as possible for that use case.
However, its data structures and algorithms are generic enough
that it should be useful for managing similar filesystem services, for example, pNFS clusters.
It is designed to integrate with existing OCF resource agents and fence agents.

#set heading(numbering: "1.")
#outline()

= Introduction

HALO consists of three main components:

- `halo_remote`: a daemon that runs on the cluster nodes;
- `halo`: a daemon that runs on the management node and performs management tasks;
- a CLI utility, which also uses the `halo` binary, that allows the admin to issue commands.

The `halo_remote` daemon performs actions on cluster nodes on behalf of the management service.
It never acts on its own--it only takes action when directed to do so by the management service.
The remote daemon uses existing OCF resource agent programs to perform management actions.

The `halo` management daemon communicates with the `halo_remote` daemons over TCP using a capnproto RPC protocol.
The daemon commands the remote agents to start, stop, and monitor resources as needed.
The management service also has the ability to fence cluster nodes by powering them off.

The CLI utility uses the `halo` binary and allows the admin to query status and perform actions on the cluster.
It communicates with the management service using an HTTP API.
Communication occurs over a unix domain socket.
The utility supports commands like `status` to display the cluster state,
and `fence` to fence a cluster node.

== Design Philosophy

HALO is designed to be as simple as possible while supporting its intended use cases.
An important part of this simplicity comes from separating the _management_ service
from the _managed_ service.

=== Non-redundant management service

Some HA management systems aim to ensure high-availibility of the management service itself,
not just the managed services that the system supports.

*High availability of the management service is a non-goal of HALO.*

The HALO management service is not redundant.
A single instance of the service runs on a single management node.
If that management node crashes, the HALO management service will stop.

This may seem to be a weakness, but in reality,
Lustre clusters are typically built with a non-redundant management node
even when using HA systems that theoretically support a redundant management service.

HALO is designed for this reality, and it is a much simpler system
by not trying to make the management service itself redundant.

=== Decoupled management and managed services

The HALO management daemon is designed so that the admin can start, stop, and restart it
without affecting the managed services.

When the management daemon stops, it does not attempt to stop the services it is responsible for.
When the daemon starts, it does not assume anything about the status of its services.
Upon starting, the daemon reaches out to all cluster nodes to discover the current state of the system
and will manage it based on the current state.

In practice this means that the management daemon is designed such that running
```bash
systemctl restart halo.service
```
on the management server is *always* safe
and never interferes with the availability of the managed filesystem.

= Configuration File

The HALO management daemon expects a configuration file in YAML format.
The config file can be specified as a CLI argument:
```bash
halo --config cluster.yaml
```
or, if it is not specified on the command line, a default location
of `/etc/halo/halo.conf` is used.

== Automatically generating a config file

The `halo discover` command is available to generate a configuration file
based on an already running cluster,
as long as certain assumptions hold true.

This command works by connecting to cluster nodes using `ssh` and running
`zpool list` and `mount -t lustre`.

Thus, for this command to generate a useful configuration,
the following conditions must be met:
- Passwordless `ssh` access from management node to cluster nodes must be set up.
- All zpools must be imported and intended to be managed by HALO.
- All lustre targets must be mounted.

The command recognizes nodeset syntax.

For example:
```bash
$ halo discover lu-mds[00-01],lu-oss[00-05]
hosts:
- hostname: lu-mds00 resources:
    mds00e0/mgt:
      kind: lustre/Lustre
      parameters:
        target: mds00e0/mgt
        mountpoint: /mnt/mgt
        kind: mgs
      requires: mds00e0
    mds00e0:
      kind: heartbeat/ZFS
      parameters:
        pool: mds00e0
      requires: null
    mds00e0/mdt:
      kind: lustre/Lustre
      parameters:
        target: mds00e0/mdt
        mountpoint: /mnt/mdt0
        kind: mdt
      requires: mds00e0
  fence_agent: null
  fence_parameters: null
...
```
The command writes out a YAML file to stdout.
The generated YAML file will typically require some manual editing to add fencing information,
since the `discover` command is not designed to detect that information.

== File Format

The YAML file consists of a list of hosts.
Each host consists of a list of resources, along with the resources' parameters,
and fencing information for the host.

=== Fencing information

Fencing information for hosts is specified in the `fence_agent` and `fence_parameters` fields.
HALO supports a set of common fence agents, for example, `fence_powerman` and `fence_redfish`.

If `fence_powerman` is used, `fence_agent` should be set to the string `"powerman"`,
and `fence_parameters` can be `null`.

If `fence_redish` is used, `fence_agent` should be set to `"redfish"`,
and `fence_parameters` needs to include `username` and `password` fields.

=== Resources

Resources are logically structured as trees based on resource dependencies.
Typically it is common for one resource, like a zpool,
to have resources that depend on it, like a lustre mgt and mdt service.
Those dependencies are specified using the `requires` field,
in which a "child" resource specifies the name of its "parent" resource.

Resources are managed using OCF Resource Agent scripts.
Those scripts expect parameters that describe the resource to be managed,
and those parameters are specified in the `parameters` field.

=== Failover Pairs

If HALO is being used to manage a cluster in which nodes are arranged in failover pairs,
those pairs must be specified in the config file:
```yaml
failover_pairs:
- - lu-mds00
  - lu-mds01
- - lu-oss00
  - lu-oss01
- - lu-oss02
  - lu-oss03
...
```

= Remote Agent

The HALO remote agent runs the `halo_remote` program.
It runs on all of the cluster nodes that host filesystem services.
It is typically managed as a systemd service called `halo-remote.service`.

== TCP endpoint

The remote agent listens for TCP connections from the management daemon.
It listens on port 8000 by default, but a different port can be specified with the `--port` command-line option.
The agent tries not to expose its TCP endpoint on any user-facing network interfaces,
and thus it does not listen on every IP address available.
By default the daemon will try to find an IP address within the network `192.168.1.0/24`
and will bind to that specific IP address.
If it cannot find an IP address in that network, it will not start.
If your cluster uses a different management network, then you must specify that network in CIDR format
using the `--network` option.

=== TLS

MTLS is available for the management daemon and remote agent to mutually authenticate each other.
It is not enabled by default because HALO is designed to be run on a secure management network.
Normally, the remote agent only listens on a private management IP address.
However, if additional security is desired, the `--mtls` option can be passed.

== OCF Resource Agents

The remote agent relies on OCF Resource Agent scripts to perform management actions.
It looks for these scripts in the default location `/usr/lib/ocf/`.
If the OCF scripts are installed in a different location, the `--ocf-root` option can be used to indicate that.

= Management Daemon

The HALO management daemon runs the `halo` program.
It is typically managed as a systemd service called `halo.service` on the cluster's management node.

== Manage versus Observe Mode

The management daemon can run in two modes:
*manage* mode in which it actively starts and stops resources, and fences nodes when needed;
and *observe* mode in which it monitors the state of the cluster but never takes action.

Currently, observe mode is the default. Manage mode must be requested using the `--manage-resources` option.
A future version of HALO will make manage mode the default.

== Unix Domain Socket

The management daemon listens for commands from the CLI utility on a unix domain socket.
The default path to the socket is `/var/run/halo.socket`, but a custom path can be specified with the `--path` option.

= CLI Utility

The CLI utility uses the `halo` binary, followed by a subcommand. For example:
```bash
$ halo failback --onto lu-oss00

$ halo status --socket halo.socket
```

The `--socket` option allows specifying the unix socket path opened by the
management daemon, in case the default location is not used.

== HTTP API

The CLI utility and the management daemon communicate with each other using an HTTP API.
In principal, this means that a tool like `curl` can be used and the CLI utility is not strictly necessary.
However, the utility is more convenient that manually making HTTP requests using curl.

== Man pages

Detailed documentation of the specific commands exists
in the command man pages, e.g., `man halo.status`.
The documentation here is a brief summary.

== commands
=== status

The `status` command is used to print out a summary of the cluster status.

=== failback

The `failback` command is used to gracefully return failed-over resourcs to their home node.
It gracefully stops the resources on their current (failover) node
and starts them on their home node once they are confirmed to be stopped.

=== manage, unmanage

The `manage` and `unmanage` commands are used to change the management status of a specific resource.
While the management daemon is running in Manage mode, 
it may be desirable to prevent it from managing some specific resources.
When a resource is unmanaged using `halo unmanage <resource_id>`,
HALO will still attempt to monitor the resource status but will not take any actions on that resource.
