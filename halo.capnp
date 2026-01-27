# SPDX-License-Identifier: MIT
# Copyright 2025. Triad National Security, LLC.

@0x9663f4dd604afa35;

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
