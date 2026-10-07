use podway_protocol::Rfc3339MillisV1;
#[test]
fn api_004_timestamp_calendar_and_clock_ranges_are_exact() {
    assert_eq!(
        Rfc3339MillisV1::from_unix_millis(0).unwrap().as_str(),
        "1970-01-01T00:00:00.000Z"
    );
    assert_eq!(
        Rfc3339MillisV1::from_unix_millis(1_800_000_300_000)
            .unwrap()
            .as_str(),
        "2027-01-15T08:05:00.000Z"
    );
    assert!(Rfc3339MillisV1::from_unix_millis(253_402_300_800_000).is_err());
    for value in [
        "2024-02-29T00:00:00.000Z",
        "2000-02-29T23:59:59.999Z",
        "0000-02-29T12:34:56.789Z",
    ] {
        assert!(Rfc3339MillisV1::new(value).is_ok(), "{value} must be valid");
    }
    for value in [
        "2023-02-29T00:00:00.000Z",
        "1900-02-29T00:00:00.000Z",
        "2026-04-31T00:00:00.000Z",
        "2026-00-01T00:00:00.000Z",
        "2026-01-00T00:00:00.000Z",
        "2026-01-01T24:00:00.000Z",
        "2026-01-01T23:60:00.000Z",
        "2026-01-01T23:59:60.000Z",
    ] {
        assert!(
            Rfc3339MillisV1::new(value).is_err(),
            "{value} must be invalid"
        );
    }
}

#[test]
fn api_004_current_error_catalog_matches_canonical_pairs_and_retires_exit_three() {
    use podway_protocol::{ErrorCodeV1, ExitCodeV1, error_code_catalog_v1};
    use std::collections::BTreeMap;
    let canonical: serde_json::Value = serde_json::from_str(include_str!(
        "../../../assets/specifications/error-codes.json"
    ))
    .unwrap();
    let expected = canonical["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["code"].as_str().unwrap().to_owned(),
                (
                    entry["exit_code"].as_u64().unwrap() as u8,
                    entry["retryable"].as_bool().unwrap(),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let actual = error_code_catalog_v1()
        .map(|(code, exit, retryable)| (code.to_owned(), (exit, retryable)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(actual, expected);
    for code in actual.keys() {
        assert!(ErrorCodeV1::new(code).is_ok());
    }
    for retired in [
        "DAEMON_UNAVAILABLE",
        "JOB_WAIT_TIMEOUT",
        "WORKSPACE_QUEUE_FULL",
    ] {
        assert!(ErrorCodeV1::new(retired).is_err());
    }
    assert!(ExitCodeV1::new(3).is_err());
}
