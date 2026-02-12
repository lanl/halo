// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

#[cfg(test)]
mod tests {
    use chrono::Local;

    use halo_lib::state::*;

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
}
