//! Intentionally broken, scroll-sized Rust diagnostics fixture.
#![allow(dead_code, unused_variables)]

#[derive(Debug, Clone)]
struct Player {
    name: String,
    health: i32,
}

impl Player {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            health: 100,
        }
    }

    fn damage(&mut self, amount: i32) {
        self.health -= amount;
    }
}

fn section_01(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 1;
    shifted
}

fn section_02(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 2;
    shifted
}

fn intentional_error_near_top() -> i32 {
    // INTENTIONAL ERROR 1: mismatched types.
    let impossible_number: i32 = "not an integer";
    impossible_number
}

fn section_03(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 3;
    shifted
}

fn section_04(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 4;
    shifted
}

fn section_05(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 5;
    shifted
}

fn section_06(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 6;
    shifted
}

fn section_07(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 7;
    shifted
}

fn section_08(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 8;
    shifted
}

fn section_09(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 9;
    shifted
}

fn section_10(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 10;
    shifted
}

fn section_11(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 11;
    shifted
}

fn section_12(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 12;
    shifted
}

fn intentional_error_near_middle() -> i32 {
    // INTENTIONAL ERROR 2: function does not exist.
    missing_scroll_fixture_function(12)
}

fn section_13(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 13;
    shifted
}

fn section_14(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 14;
    shifted
}

fn section_15(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 15;
    shifted
}

fn section_16(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 16;
    shifted
}

fn section_17(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 17;
    shifted
}

fn section_18(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 18;
    shifted
}

fn section_19(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 19;
    shifted
}

fn section_20(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 20;
    shifted
}

fn section_21(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 21;
    shifted
}

fn section_22(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 22;
    shifted
}

fn intentional_error_lower_middle(player: &Player) -> i32 {
    // INTENTIONAL ERROR 3: Player has no `speed` field.
    player.speed
}

fn section_23(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 23;
    shifted
}

fn section_24(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 24;
    shifted
}

fn section_25(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 25;
    shifted
}

fn section_26(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 26;
    shifted
}

fn section_27(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 27;
    shifted
}

fn section_28(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 28;
    shifted
}

fn section_29(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 29;
    shifted
}

fn section_30(input: i32) -> i32 {
    let doubled = input * 2;
    let shifted = doubled + 30;
    shifted
}

fn intentional_error_near_bottom(player: &mut Player) {
    // INTENTIONAL ERROR 4: `damage` takes exactly one argument.
    player.damage(5);
}

fn main() {
    let mut player = Player::new("scroll tester");
    let total = section_01(1) + section_15(15) + section_30(30);
    player.damage(total);
    println!("{player:?}");
}
