// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use halo_lib::{cluster::Cluster, manager};

    #[test]
    fn failover_partners() {
        let config_path = halo_lib::test_env::test_path("configs/lustre.yaml");
        let args = manager::Cli {
            config: Some(config_path),
            use_insecure_port: true,
            ..Default::default()
        };
        let cluster = Cluster::new(args).unwrap();

        let first_host = cluster.hosts().next().unwrap();
        let first_host_partner = Arc::clone(first_host);
        let partner_set_res = first_host.set_failover_partner(Some(first_host_partner));
        assert_eq!(
            partner_set_res,
            halo_lib::HandledResult::Err(halo_lib::HandledError {})
        );
    }
}
