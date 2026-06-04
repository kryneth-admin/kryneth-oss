use simd_json::prelude::*;

fn main() {
    let json_str = r#"{"model": "test"}"#;
    let mut bytes = json_str.as_bytes().to_vec();
    let mut value = simd_json::to_owned_value(&mut bytes).unwrap();
    if let Some(obj) = value.as_object_mut() {
        obj.insert("model".into(), simd_json::OwnedValue::from("mutated"));
    }
    let serialized = serde_json::to_string(&value).unwrap();
    println!("{}", serialized);
}
