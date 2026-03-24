use fastarena::{arenavec, Arena, ArenaVec};

#[test]
fn macro_empty() {
    let mut arena = Arena::new();
    let v: ArenaVec<u32> = arenavec![in &mut arena];
    assert!(v.is_empty());
    assert_eq!(v.len(), 0);
}

#[test]
fn macro_list() {
    let mut arena = Arena::new();
    let v = arenavec![in &mut arena; 1u32, 2, 3];
    assert_eq!(v.len(), 3);
    assert_eq!(v[0], 1);
    assert_eq!(v[1], 2);
    assert_eq!(v[2], 3);
}

#[test]
fn macro_list_trailing_comma() {
    let mut arena = Arena::new();
    let v = arenavec![in &mut arena; 10u64, 20, 30,];
    assert_eq!(v.as_slice(), &[10, 20, 30]);
}

#[test]
fn macro_list_single_element() {
    let mut arena = Arena::new();
    let v = arenavec![in &mut arena; 42u8];
    assert_eq!(v.len(), 1);
    assert_eq!(v[0], 42);
}

#[test]
fn macro_repeat() {
    let mut arena = Arena::new();
    let v = arenavec![in &mut arena; 7u32; 5];
    assert_eq!(v.len(), 5);
    assert_eq!(v.as_slice(), &[7, 7, 7, 7, 7]);
}

#[test]
fn macro_repeat_zero() {
    let mut arena = Arena::new();
    let v = arenavec![in &mut arena; 0u32; 0];
    assert!(v.is_empty());
}

#[test]
fn macro_repeat_expression_count() {
    let mut arena = Arena::new();
    let n = 8usize;
    let v = arenavec![in &mut arena; 1u32; n];
    assert_eq!(v.len(), 8);
}

#[test]
fn macro_list_with_finish() {
    let mut arena = Arena::new();
    let slice = arenavec![in &mut arena; 1u32, 2, 3].finish();
    assert_eq!(slice, &mut [1, 2, 3]);
}

#[test]
fn macro_with_transaction() {
    let mut arena = Arena::new();
    let mut txn = arena.transaction();
    {
        let v = arenavec![in txn.arena_mut(); 10u32, 20, 30];
        assert_eq!(v.as_slice(), &[10, 20, 30]);
    }
    txn.commit();
}

#[test]
fn macro_strings() {
    let mut arena = Arena::new();
    let v = arenavec![in &mut arena; "hello".to_string(), "world".to_string()];
    assert_eq!(v[0], "hello");
    assert_eq!(v[1], "world");
}

#[test]
fn macro_repeat_string() {
    let mut arena = Arena::new();
    let v = arenavec![in &mut arena; "x".to_string(); 3];
    assert_eq!(v.as_slice(), &["x", "x", "x"]);
}
