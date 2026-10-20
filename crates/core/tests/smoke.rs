#[test]
fn core_links() {
    assert_eq!(shabdly_core::VERSION, env!("CARGO_PKG_VERSION"));
}
