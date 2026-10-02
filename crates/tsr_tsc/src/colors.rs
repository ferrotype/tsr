//! Terminal colors used by the pinned command-line help renderer.
use crate::System;

pub(crate) struct Colors {
    show_colors: bool,
    is_windows: bool,
    is_windows_terminal: bool,
    is_vscode: bool,
    supports_richer_colors: bool,
}

// port: tsc/internal/execute/tsc/diagnostics.go:createColors
pub(crate) fn create_colors(sys: &dyn System) -> Colors {
    if !crate::default_is_pretty(sys) {
        return Colors {
            show_colors: false,
            is_windows: false,
            is_windows_terminal: false,
            is_vscode: false,
            supports_richer_colors: false,
        };
    }
    let env = |name| sys.get_environment_variable(name).unwrap_or_default();
    let os = tsr_jsstring::helpers::to_lower_go(env("OS").as_bytes());
    Colors {
        show_colors: true,
        is_windows: os.windows(b"windows".len()).any(|w| w == b"windows"),
        is_windows_terminal: !env("WT_SESSION").is_empty(),
        is_vscode: env("TERM_PROGRAM").as_bytes() == b"vscode",
        supports_richer_colors: env("COLORTERM").as_bytes() == b"truecolor"
            || env("TERM").as_bytes() == b"xterm-256color",
    }
}

impl Colors {
    fn wrap(&self, text: &[u8], start: &[u8], end: &[u8]) -> Vec<u8> {
        if self.show_colors {
            [start, text, end].concat()
        } else {
            text.to_vec()
        }
    }

    // port: tsc/internal/execute/tsc/diagnostics.go:colors.bold
    pub(crate) fn bold(&self, text: &[u8]) -> Vec<u8> {
        self.wrap(text, b"\x1b[1m", b"\x1b[22m")
    }

    // port: tsc/internal/execute/tsc/diagnostics.go:colors.blue
    pub(crate) fn blue(&self, text: &[u8]) -> Vec<u8> {
        if self.is_windows && !self.is_windows_terminal && !self.is_vscode {
            self.bright_white(text)
        } else {
            self.wrap(text, b"\x1b[94m", b"\x1b[39m")
        }
    }

    // port: tsc/internal/execute/tsc/diagnostics.go:colors.blueBackground
    pub(crate) fn blue_background(&self, text: &[u8]) -> Vec<u8> {
        self.wrap(
            text,
            if self.supports_richer_colors {
                b"\x1b[48;5;68m"
            } else {
                b"\x1b[44m"
            },
            b"\x1b[39;49m",
        )
    }

    // port: tsc/internal/execute/tsc/diagnostics.go:colors.brightWhite
    pub(crate) fn bright_white(&self, text: &[u8]) -> Vec<u8> {
        self.wrap(text, b"\x1b[97m", b"\x1b[39m")
    }
}
