use rune::Env;

// External consumers construct and exhaustively destructure this exact two-field API.
// Adding even a private field must fail this integration test's compilation.
const ENV: Env = Env { system_dark: true, scale: 2.0 };

#[test]
fn env_keeps_its_exhaustive_literal_api() {
    let Env { system_dark, scale } = ENV;
    assert!(system_dark);
    assert_eq!(scale, 2.0);
}
