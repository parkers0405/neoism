NEOISM(1)

# NAME

Neoism - A cross-platform, GPU-accelerated terminal emulator.

# SYNOPSIS

Neoism is a modern terminal emulator built with Rust and WebGPU that provides high performance rendering with support for modern terminal features including sixel graphics, hyperlinks, and custom shaders.

# FLAGS

*-h, --help*

	Prints help information.

*-V, --version*

	Prints version information.

# OPTIONS

*-e, --command* _<COMMAND>..._

	Command and args to execute (must be last argument).

*-w, --working-dir* _<WORKING_DIR>_

	Start the shell in the specified working directory.

*--write-config* _[<PATH>]_

	Writes the config to a given path or the default location.

	The default config location is selected in this order on Linux:

		. _$NEOISM_CONFIG_HOME/config.json_
		. _$XDG_CONFIG_HOME/neoism/config.json_
		. _$HOME/.config/neoism/config.json_

	On macOS, use _$NEOISM_CONFIG_HOME/config.json_ when set, otherwise _$HOME/.config/neoism/config.json_. On Windows, _$NEOISM_CONFIG_HOME/config.json_ also takes precedence over the default:

		. _%USERPROFILE%\\AppData\\Local\\neoism\\config.json_

*--enable-log-file*

	Writes the logs to a file inside the config directory.

*--title-placeholder* _<TITLE>_

	Start window with specified title.

# CONFIGURATION

Neoism uses JSONC (JSON with comments and trailing commas) for configuration. The configuration file is automatically reloaded when changed. See *neoism*(5) for detailed configuration documentation.

# FEATURES

Neoism supports many modern terminal features:

- GPU-accelerated rendering with WebGPU
- Sixel graphics protocol
- iTerm2 image protocol
- Kitty keyboard protocol
- Hyperlinks (OSC 8)
- Vi mode for navigation
- Split panels
- Custom shaders (RetroArch compatible)
- Adaptive themes
- Font ligatures
- Wide color gamut support

# ENVIRONMENT

*NEOISM_CONFIG_HOME*

	Custom path for Neoism configuration directory. When set, Neoism will look for
	configuration files in this directory instead of the default locations.

# SEE ALSO

*neoism*(5)

# BUGS

Found a bug? Please report it at _https://github.com/parkers0405/neoism/issues_.

# MAINTAINERS

- Raphael Amorim <rapha850@gmail.com>
