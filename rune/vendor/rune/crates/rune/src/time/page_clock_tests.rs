use super::*;

#[test]
fn a_clock_that_jumps_back_continues_from_its_last_reading() {
    let mut clock = Monotonic::new();
    assert_eq!(clock.read(3500.0), 3500.0);
    // A fake clock installed late starts over near zero; time carries on from 3500.
    assert_eq!(clock.read(2000.0), 3500.0);
    assert_eq!(clock.read(2016.0), 3516.0);
    assert_eq!(clock.read(2016.0), 3516.0);
    assert_eq!(clock.read(f64::NAN), 3516.0);
    // Forward jumps (a fake clock's pause or fast-forward) pass through.
    assert_eq!(clock.read(3016.0), 4516.0);
}

#[test]
fn instants_do_arithmetic_like_std() {
    let a = Instant(Duration::from_millis(1000));
    let b = a + Duration::from_millis(250);
    assert_eq!(b - a, Duration::from_millis(250));
    assert_eq!(a - b, Duration::ZERO);
    assert_eq!(a.checked_duration_since(b), None);
    assert_eq!(b.checked_sub(Duration::from_millis(250)), Some(a));
    assert_eq!(a.checked_sub(Duration::from_secs(2)), None);
    let mut c = a;
    c += Duration::from_millis(5);
    c -= Duration::from_millis(5);
    assert_eq!(c, a);
    assert!(a < b);
}
