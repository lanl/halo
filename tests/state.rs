// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::Local;

    use halo_lib::{state::*, test_env::*};

    fn test_env_helper(test_id: &str) -> HaEnvironment {
        HaEnvironment::new(
            test_id.to_string(),
            env!("CARGO_BIN_EXE_halo_remote"),
            env!("CARGO_BIN_EXE_halo_manager"),
        )
    }

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
        let cluster = env.cluster();

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

        let cluster = env.cluster();

        let host = cluster.get_host("127.0.0.1").unwrap();
        assert!(!host.active());
        let rg = cluster.get_resource_group("test_zpool");
        assert!(!rg.get_managed());
    }

    #[test]
    fn restart_fence() {
        let env = test_env_helper("restart_fence");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(_b);
        std::thread::sleep(std::time::Duration::from_secs(1));

        let _m = env.restart_manager(_m);
        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("1") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
        for host in cluster_status.hosts {
            if host.id.ends_with("1") {
                assert!(host.fenced);
                assert!(!host.connected)
            } else {
                assert!(!host.fenced)
            }
        }
    }

    #[test]
    fn restart_fence2() {
        let env = test_env_helper("restart_fence2");
        let _a = env.start_agent(0);
        let _b = env.start_agent(1);
        let _m = env.start_manager(true);

        std::thread::sleep(std::time::Duration::from_secs(1));
        drop(_b);
        std::thread::sleep(std::time::Duration::from_secs(1));

        drop(_m);
        // While manager is stopped, manually fail back resources...
        env.stop_resource("mdt_1", 0);
        env.stop_resource("zpool_1", 0);
        env.start_resource("zpool_1", 1);
        env.start_resource("mdt_1", 1);

        let _m = env.start_manager(true);
        std::thread::sleep(std::time::Duration::from_secs(1));

        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("1") {
                assert_eq!(res.status, "Error");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
        for host in cluster_status.hosts {
            if host.id.ends_with("1") {
                assert!(host.fenced);
                assert!(!host.connected)
            } else {
                assert!(!host.fenced);
                assert!(host.connected)
            }
        }

        // now fence manually:
        env.fence(1, true).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let cluster_status = env.get_status();
        for res in cluster_status.resources {
            if res.id.contains("1") {
                assert_eq!(res.status, "Running (Failed Over)");
            } else {
                assert_eq!(res.status, "Running");
            }
        }
    }
}
