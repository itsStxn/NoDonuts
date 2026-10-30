// Integration tests test your crate's public API. They only have access to items
// in your crate that are marked pub. See the Cargo Targets page of the Cargo Book
// for more information.
//
//   https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests
//
#[test]
fn integration_test() {
    use amzn_no_donuts::hello;

    assert_eq!(hello("Brazil"), "Hello new friend: Brazil");
}
