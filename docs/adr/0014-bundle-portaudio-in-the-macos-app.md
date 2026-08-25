# Bundle PortAudio in the macOS application

The macOS `.app` will carry the PortAudio runtime library in `Contents/Frameworks` and rewrite its install name, rather than requiring end users to install Homebrew dependencies. Developers still install PortAudio and `pkgconf` to build; bundling adds packaging work but makes the accepted version-one artifact self-contained and prepares every native library for later signing as one unit.
