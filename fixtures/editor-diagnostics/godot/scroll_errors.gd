extends Node

# Intentionally broken, scroll-sized GDScript fixture.
# Neoism currently has no bundled GDScript LSP, so this file must NOT inherit
# Rust or any other language server merely because the parent repository uses it.

class_name DiagnosticsScrollFixture

var frame_total: int = 0
var player_name: String = "scroll tester"
var samples: Array[int] = []

func _ready() -> void:
	seed_samples()
	print(build_label(1))

func seed_samples() -> void:
	for value in range(24):
		samples.append(value)

func build_label(index: int) -> String:
	return "%s:%d" % [player_name, index]

func section_01(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 1
	frame_total += shifted
	return shifted

func section_02(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 2
	frame_total += shifted
	return shifted

func section_03(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 3
	frame_total += shifted
	return shifted

func section_04(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 4
	frame_total += shifted
	return shifted

func intentional_error_near_top() -> int:
	# INTENTIONAL ERROR 1: String cannot initialize an int.
	var impossible_number: int = "not an integer"
	return impossible_number

func section_05(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 5
	frame_total += shifted
	return shifted

func section_06(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 6
	frame_total += shifted
	return shifted

func section_07(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 7
	frame_total += shifted
	return shifted

func section_08(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 8
	frame_total += shifted
	return shifted

func section_09(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 9
	frame_total += shifted
	return shifted

func section_10(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 10
	frame_total += shifted
	return shifted

func section_11(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 11
	frame_total += shifted
	return shifted

func section_12(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 12
	frame_total += shifted
	return shifted

func intentional_error_near_middle() -> int:
	# INTENTIONAL ERROR 2: this identifier does not exist.
	return missing_scroll_fixture_value + 12

func section_13(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 13
	frame_total += shifted
	return shifted

func section_14(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 14
	frame_total += shifted
	return shifted

func section_15(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 15
	frame_total += shifted
	return shifted

func section_16(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 16
	frame_total += shifted
	return shifted

func section_17(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 17
	frame_total += shifted
	return shifted

func section_18(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 18
	frame_total += shifted
	return shifted

func section_19(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 19
	frame_total += shifted
	return shifted

func section_20(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 20
	frame_total += shifted
	return shifted

func section_21(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 21
	frame_total += shifted
	return shifted

func section_22(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 22
	frame_total += shifted
	return shifted

func section_23(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 23
	frame_total += shifted
	return shifted

func section_24(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 24
	frame_total += shifted
	return shifted

func section_25(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 25
	frame_total += shifted
	return shifted

func section_26(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 26
	frame_total += shifted
	return shifted

func section_27(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 27
	frame_total += shifted
	return shifted

func section_28(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 28
	frame_total += shifted
	return shifted

func section_29(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 29
	frame_total += shifted
	return shifted

func section_30(value: int) -> int:
	var doubled := value * 2
	var shifted := doubled + 30
	frame_total += shifted
	return shifted

func intentional_error_near_bottom() -> Vector3:
	# INTENTIONAL ERROR 3: int cannot be returned as Vector3.
	return 42

func summarize() -> String:
	var total := 0
	for value in samples:
		total += value
	return "frames=%d samples=%d total=%d" % [frame_total, samples.size(), total]
