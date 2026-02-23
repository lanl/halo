// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::Local;

    use halo_lib::{
        state::*,
        test_env::*,
    };

    #[test]
    fn serde1() {
        let record = Record {
            timestamp: Local::now().naive_local(),
            event: Event::Manage,
            obj_id: "ost00".to_string(),
            comment: Some("hello".to_string()),
        };
        let output = record.as_string();

        let new_record = Record::from_string(&output).unwrap();

        assert_eq!(record, new_record);
    }

    #[test]
    fn state() {
        let env = TestEnvironment::new(
            "state".to_string(),
            env!("CARGO_BIN_EXE_halo_remote"),
            env!("CARGO_BIN_EXE_halo_manager"),
        );
        let cluster = env.cluster(None);

        let now = Local::now().naive_local();
        let records: Vec<Record> = vec![
            Record {
                timestamp: now - Duration::from_secs(20),
                event: Event::Activate,
                obj_id: "127.0.0.1".to_string(),
                comment: Some("admin managing 127.0.0.1".to_string()),
            },
            Record {
                timestamp: now - Duration::from_secs(15),
                event: Event::Deactivate,
                obj_id: "127.0.0.1".to_string(),
                comment: Some("admin deactivating 127.0.0.1".to_string()),
            },
            Record {
                timestamp: now - Duration::from_secs(10),
                event: Event::Unmanage,
                obj_id: "test_zpool".to_string(),
                comment: Some("admin unmanaging test_zpool".to_string()),
            },
        ];
        for record in records {
            let _ = cluster.write_record(record);
        }

        let cluster = env.cluster(None);

        for host in cluster.hosts() {
            assert!(host.active());
            assert!(!host.fenced());
        }
        for rg in cluster.resource_groups() {
            assert!(rg.get_managed());
        }

        cluster.apply_state();

        let host = cluster.get_host("127.0.0.1").unwrap();
        assert!(!host.active());
        let rg = cluster.get_resource_group("test_zpool");
        assert!(!rg.get_managed());
    }
}
