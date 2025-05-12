// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use futures::future;

use crate::cluster;
use crate::commands;
use crate::resource;

pub async fn start(cluster: cluster::Cluster) -> commands::Result {
    // 1. All zpools.
    let zpool_statuses: Vec<_> = cluster
        .zpool_resources()
        .map(|z| async {
            (
                z.parameters.clone(),
                z.start(resource::Location::Home).await,
            )
        })
        .collect();

    let results = future::join_all(zpool_statuses).await;
    results.iter().for_each(|r| println!("{:?}", r));

    // 2. Lustre MGS target.
    let mgs = cluster.get_mgs();
    match mgs {
        Some(mgs) => {
            let status = mgs.start(resource::Location::Home).await;
            println!("{:?}", ("mgs", status));
        }
        None => eprintln!("Could not find mgs target."),
    };

    // 3. All remaining Lustre targets.
    let target_statuses: Vec<_> = cluster
        .lustre_resources_no_mgs()
        .map(|t| async {
            (
                t.parameters.clone(),
                t.start(resource::Location::Home).await,
            )
        })
        .collect();

    let results = future::join_all(target_statuses).await;
    results.iter().for_each(|r| println!("{:?}", r));

    Ok(())
}
