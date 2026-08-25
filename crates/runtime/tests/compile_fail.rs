//! Compile-fail cases. These assert properties no runtime test can reach: that
//! certain programs do not build at all.

#[test]
fn ui() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
