#[path = "int_build_identity_git_refs.rs"]
mod int_build_identity_git_refs;
#[path = "int_machine_contract_fixtures.rs"]
mod int_machine_contract_fixtures;
#[path = "int_phase4_framing.rs"]
mod int_phase4_framing;
#[path = "int_phase4_slice_contract.rs"]
mod int_phase4_slice_contract;
#[path = "int_phase5_slice_contract.rs"]
mod int_phase5_slice_contract;
#[cfg(feature = "release-contract-verifier")]
#[path = "int_release_contract_verifier.rs"]
mod int_release_contract_verifier;
#[path = "int_v2_protocol_requests.rs"]
mod int_v2_protocol_requests;
#[path = "int_v2_response_codec.rs"]
mod int_v2_response_codec;
#[path = "int_v2_result_contract.rs"]
mod int_v2_result_contract;
