// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::collections::HashMap;

use crate::{config::*, manager::Cli, HandledResult};

pub fn test_config() -> HandledResult<()> {
    let hosts = vec![
        Host2 {
            hostname: "nfs1".to_string(),
            fence_agent: None,
            fence_parameters: None,
        },
        Host2 {
            hostname: "nfs2".to_string(),
            fence_agent: None,
            fence_parameters: None,
        },
    ];

    let mut resources = Vec::new();
    for i in 0..2 {
        let res = Resource2 {
            name: format!("ip_{}", i + 1),
            kind: "ip addr".to_string(),
            parameters: HashMap::from([("address".to_string(), format!("192.168.100.{i}"))]),
            dependents: vec!["mount_1".to_string(), "mount_2".to_string()],
        };
        resources.push(res);

        let res = Resource2 {
            name: format!("mount_{}", i + 1),
            kind: "filesystem".to_string(),
            parameters: HashMap::from([
                ("kind".to_string(), "nfs".to_string()),
                ("location".to_string(), "10.0.0.10:/export".to_string()),
            ]),
            dependents: Vec::new(),
        };
        resources.push(res);
    }

    let resource_groups = vec![
        ResourceGroup {
            home_host: "nfs1".to_string(),
            failover_hosts: vec!["nfs2".to_string()],
            root: "ip_1".to_string(),
        },
        ResourceGroup {
            home_host: "nfs1".to_string(),
            failover_hosts: vec!["nfs2".to_string()],
            root: "ip_2".to_string(),
        },
    ];

    let config = Config2 {
        hosts,
        resources,
        resource_groups,
    };

    println!("{}", serde_yaml::to_string(&config).unwrap());
    crate::cluster::Cluster::from_config2(&config, Cli::default(), None, None);

    Ok(())
}
