// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

/// Usability tests
///
/// The purpose of these tests is to ensure that running the binaries with invalid arguments or
/// missing files (i.e., missing configuration files) results in a failing exit status and a useful
/// error message.

#[cfg(test)]
mod tests {
    #[test]
    fn remote() {
        let invalid_network = "127.12.34.0/24";
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_halo_remote"))
            .args(vec!["--network", invalid_network])
            .output()
            .unwrap();

        assert!(!result.status.success());
        let err_message = String::from_utf8(result.stderr).unwrap();
        assert!(err_message.contains(invalid_network));
    }

    #[test]
    fn manager_config() {
        let invalid_config = "this_file_does_not_exist";
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_halo"))
            .args(vec!["--config", invalid_config])
            .output()
            .unwrap();

        assert!(!result.status.success());
        let err_message = String::from_utf8(result.stderr).unwrap();
        assert!(err_message.contains(invalid_config));
    }

    #[test]
    fn manager_socket() {
        let good_config_path = format!(
            "{}/{}",
            std::env::var("CARGO_MANIFEST_DIR").unwrap(),
            "tests/simple.toml"
        );
        let invalid_socket = "bad_dir/socket";
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_halo"))
            .args(vec![
                "--config",
                &good_config_path,
                "--socket",
                invalid_socket,
            ])
            .output()
            .unwrap();

        assert!(!result.status.success());
        let err_message = String::from_utf8(result.stderr).unwrap();
        assert!(err_message.contains(invalid_socket));
    }

    #[test]
    fn discover_nodeset() {
        let invalid_nodesets = ["node[0-2", "node[a]", "host0[-1]"];
        for invalid_nodeset in invalid_nodesets.iter() {
            let result = std::process::Command::new(env!("CARGO_BIN_EXE_halo"))
                .args(vec!["discover", invalid_nodeset])
                .output()
                .unwrap();

            assert!(!result.status.success());
            let err_message = String::from_utf8(result.stderr).unwrap();
            assert!(err_message.contains(invalid_nodeset));
        }
    }
}
