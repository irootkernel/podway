#![no_main]

use libfuzzer_sys::fuzz_target;
use podway_config::{CanonicalDigest, CanonicalJson, MAX_WORKSPACE_CONFIG_BYTES_V1, parse_workspace_config_v1};
use podway_core::{canonicalize_json_v1, verify_canonical_json_v1};
use serde_json::Value;

fn expected_canonical(value: &Value, output: &mut Vec<u8>) -> Result<(), serde_json::Error> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => output.extend_from_slice(if *value { &b"true"[..] } else { &b"false"[..] }),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() { output.extend_from_slice(value.to_string().as_bytes()); }
            else if let Some(value) = value.as_u64() { output.extend_from_slice(value.to_string().as_bytes()); }
            else { return Err(serde_json::Error::io(std::io::Error::other("floating point number"))); }
        }
        Value::String(value) => serde_json::to_writer(&mut *output, value)?,
        Value::Array(values) => { output.push(b'['); for (index, value) in values.iter().enumerate() { if index != 0 { output.push(b','); } expected_canonical(value, output)?; } output.push(b']'); }
        Value::Object(values) => { let mut entries: Vec<_> = values.iter().collect(); entries.sort_unstable_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes())); output.push(b'{'); for (index, (key, value)) in entries.into_iter().enumerate() { if index != 0 { output.push(b','); } serde_json::to_writer(&mut *output, key)?; output.push(b':'); expected_canonical(value, output)?; } output.push(b'}'); }
    }
    Ok(())
}

fn sha256_hex(input: &[u8]) -> String {
    const K: [u32; 64] = [0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2];
    let bit_len = (input.len() as u64).wrapping_mul(8); let mut bytes = input.to_vec(); bytes.push(0x80); while bytes.len() % 64 != 56 { bytes.push(0); } bytes.extend_from_slice(&bit_len.to_be_bytes());
    let mut h = [0x6a09e667u32,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19];
    for block in bytes.chunks_exact(64) { let mut w = [0u32; 64]; for (i, chunk) in block.chunks_exact(4).enumerate() { w[i] = u32::from_be_bytes(chunk.try_into().unwrap()); } for i in 16..64 { w[i] = w[i-16].wrapping_add(w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3)).wrapping_add(w[i-7]).wrapping_add(w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10)); } let (mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut x)=(h[0],h[1],h[2],h[3],h[4],h[5],h[6],h[7]); for i in 0..64 { let s1=e.rotate_right(6)^e.rotate_right(11)^e.rotate_right(25); let ch=(e&f)^(!e&g); let t1=x.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]); let s0=a.rotate_right(2)^a.rotate_right(13)^a.rotate_right(22); let maj=(a&b)^(a&c)^(b&c); x=g;g=f;f=e;e=d.wrapping_add(t1);d=c;c=b;b=a;a=t1.wrapping_add(s0).wrapping_add(maj); } for (state, value) in h.iter_mut().zip([a,b,c,d,e,f,g,x]) { *state = state.wrapping_add(value); } }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

fuzz_target!(|input: &[u8]| {
    let canonical_input = &input[..input.len().min(MAX_WORKSPACE_CONFIG_BYTES_V1)];
    if let Ok(value) = serde_json::from_slice::<Value>(canonical_input) {
        let mut expected = Vec::new();
        let expected = expected_canonical(&value, &mut expected).ok().and_then(|_| String::from_utf8(expected).ok());
        assert_eq!(canonicalize_json_v1(&value).ok().as_deref(), expected.as_deref());
        if let Some(expected) = expected { assert_eq!(verify_canonical_json_v1(expected.as_bytes()), Ok(())); }
    }
    assert!(verify_canonical_json_v1(br#"{"b":1,"a":2}"#).is_err());
    assert!(verify_canonical_json_v1(b"1.0").is_err());
    assert!(verify_canonical_json_v1(b"{").is_err());

    if let Ok(config) = parse_workspace_config_v1(input) {
        let canonical = config.canonical_json_v1().expect("validated workspace config must canonicalize");
        let digest = config.canonical_digest_v1().expect("validated workspace config must have a digest");
        let reparsed = serde_json::from_slice::<Value>(canonical.as_bytes()).expect("canonical config must be JSON");
        let mut expected = Vec::new(); expected_canonical(&reparsed, &mut expected).expect("canonical config uses integer JSON");
        assert_eq!(canonical.as_bytes(), expected.as_slice());
        assert_eq!(digest.as_str(), format!("sha256:{}", sha256_hex(canonical.as_bytes())));
        assert_eq!(verify_canonical_json_v1(canonical.as_bytes()), Ok(()));
    }
});
