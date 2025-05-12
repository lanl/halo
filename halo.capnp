# SPDX-License-Identifier: MIT
# Copyright 2025. Triad National Security, LLC.

@0x9663f4dd604afa35;

interface HaloMgmt {
    # The interface for communication between the CLI tool and the management daemon.

    enum Status {
        unknown @0;
        checkingHome @1;
        runningOnHome @2;
        stopped @3;
        checkingAway @4;
        runningOnAway @5;
        unrunnable @6;
    }

    struct Cluster {
        resources @0 :List(Resource);
    }

    struct Resource {
        parameters @0 :List(Parameter);
        struct Parameter {
            key @0 :Text;
            value @1 :Text;
        }
	status @1 :Status;
    }

    monitor @0 () -> (status: Cluster);
}

interface OcfResourceAgent {
    # The interface for sending commnds to OCF Resource Agents.
    enum Operation {
        monitor @0;
        start @1;
        stop @2;
    }

    struct Argument {
        key @0 :Text;
        value @1 :Text;
    }

    struct Result {
        union {
            ok @0 :Int32;
            err @1 :Text;
        }
    }

    operation @0 (resource :Text, op :Operation, args :List(Argument)) -> (result :Result);
}
