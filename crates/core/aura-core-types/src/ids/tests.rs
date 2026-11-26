use super::*;

#[test]
fn ids_roundtrip_hex() {
    let id = TurnId::generate();
    let hex = id.to_hex();
    let back = TurnId::from_hex(&hex).unwrap();
    assert_eq!(id, back);
}

#[test]
fn ids_have_stable_serde() {
    let id = SessionId::new([7u8; 16]);
    let json = serde_json::to_string(&id).unwrap();
    let back: SessionId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, back);
}

#[test]
fn transaction_id_hashes_uuid_bytes() {
    let uuid = uuid::Uuid::new_v4();
    let id = TransactionId::from_uuid(uuid);
    assert_ne!(id, TransactionId::default());
}
