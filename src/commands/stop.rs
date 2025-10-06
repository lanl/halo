// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use futures::future;

use crate::{cluster, commands::HandledResult};

pub async fn stop(cluster: cluster::Cluster) -> HandledResult<()> {
    // 1. All Lustre targets but MGS.
    let target_statuses: Vec<_> = cluster
        .lustre_resources_no_mgs()
        .map(|t| async { (t.parameters.clone(), t.stop().await) })
        .collect();

    let results = future::join_all(target_statuses).await;
    results.iter().for_each(|r| println!("{:?}", r));

    // 2. Lustre MGS target.
    let mgs = cluster.get_mgs();
    match mgs {
        Some(mgs) => {
            let status = mgs.stop().await;
            println!("{:?}", ("mgs", status));
        }
        None => eprintln!("Could not find mgs target."),
    };

    // 1. All zpools.
    let zpool_statuses: Vec<_> = cluster
        .zpool_resources()
        .map(|z| async { (z.parameters.clone(), z.stop().await) })
        .collect();

    let results = future::join_all(zpool_statuses).await;
    results.iter().for_each(|r| println!("{:?}", r));

    Ok(())
}
