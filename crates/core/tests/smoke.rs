#[test]
fn core_links() {
    assert_eq!(voozi_core::VERSION, env!("CARGO_PKG_VERSION"));
}
