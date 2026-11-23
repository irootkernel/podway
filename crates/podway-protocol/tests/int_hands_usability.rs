use podway_protocol::decode_group_activate_input_v1;
use serde_json::json;

#[test]
fn hands001_schema_failure_identifies_the_first_bounded_placeholder() {
    let document = json!({
        "schema": "podway.group-activate-input/v1",
        "a~/key": [{"z": "<first>"}, "<second>"],
        "z": "<later>"
    });
    let failure = decode_group_activate_input_v1(document.to_string().as_bytes()).unwrap_err();
    assert_eq!(
        failure.to_string(),
        "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\" at /a~0~1key/0/z"
    );
}

#[test]
fn hands001_schema_failure_without_a_placeholder_names_only_the_schema() {
    let document = json!({
        "schema": "podway.group-activate-input/v1",
        "context": [format!("<{}>", "x".repeat(63)), "private-value"]
    });
    let failure = decode_group_activate_input_v1(document.to_string().as_bytes()).unwrap_err();
    assert_eq!(
        failure.to_string(),
        "invalid successor input document podway.group-activate-input/v1: the document does not match its schema"
    );
}

#[test]
fn hands001_schema_failure_preserves_decoded_pointer_characters() {
    for key in ["row\nname", r"row\nname"] {
        let document = json!({
            "schema": "podway.group-activate-input/v1",
            (key): "<first>"
        });
        let failure = decode_group_activate_input_v1(document.to_string().as_bytes()).unwrap_err();
        assert_eq!(
            failure.to_string(),
            format!(
                "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\" at /{key}"
            )
        );
    }
}

#[test]
fn hands001_schema_failure_keeps_the_token_when_the_pointer_is_too_long() {
    let key = "x".repeat(65_536);
    let document = json!({
        "schema": "podway.group-activate-input/v1",
        (key): "<first>",
        "z": "<later>"
    });
    let failure = decode_group_activate_input_v1(document.to_string().as_bytes()).unwrap_err();
    assert_eq!(
        failure.to_string(),
        "invalid successor input document podway.group-activate-input/v1: replace template placeholder \"<first>\" (RFC 6901 pointer exceeds the error message length limit)"
    );
}
