// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    #[test]
    fn failover_partners() {
        let config_path = halo_lib::test_env::test_path("configs/lustre.yaml");
        let cluster = halo_lib::cluster::Cluster::from_config(Some(config_path)).unwrap();

        let first_host = cluster.hosts().next().unwrap();
        let first_host_partner = Arc::clone(first_host);
        let partner_set_res = first_host.set_failover_partner(Some(first_host_partner));
        assert_eq!(
            partner_set_res,
            halo_lib::HandledResult::Err(halo_lib::HandledError {})
        );
    }
}
